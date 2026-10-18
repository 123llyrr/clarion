#!/usr/bin/env python3
"""Validate GPUI file-chooser parents on an owned compositor and fake portal.

Build: cargo build -p clarion-shell --example portal_parent_probe --offline -j2
The probe requires actual keyboard focus before asking for a chooser. Its fake
FileChooser records the real D-Bus parent and returns Cancel without user input.
Neither the portal nor any Wayland client connects to the parent desktop bus.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location("native_service_driver", ROOT / "scripts/test-native-shell-services.py")
_driver = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_driver)
require, stop, wait_for, run = _driver.require, _driver.stop, _driver.wait_for, _driver.run


def exercise(args):
    private = args.private_root.resolve()
    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    runtime = Path(env["XDG_RUNTIME_DIR"])
    require(runtime == private / "runtime", "not the owned runtime")
    require(env.get("DBUS_SESSION_BUS_ADDRESS", "").startswith(f"unix:path={runtime}/"), "not the private portal bus")
    parent = Path(env["CLARION_PORTAL_PARENT_WAYLAND"])
    require(parent.is_absolute() and parent.is_socket() and parent.parent != runtime, "invalid parent display")
    report = {"passed": False, "cleanup": False, "scope": "private compositor and fake FileChooser portal", "hostPortalAccess": False}
    core = portal = probe = None
    logs = []
    try:
        config = private / "nested.kdl"
        config.write_text('input { keyboard { xkb { layout "us"; }; }; }\noutput "winit" { scale 1; }\n'
                          'xwayland-satellite { off; }\nhotkey-overlay { skip-at-startup; }\n')
        nested_env = env | {"WAYLAND_DISPLAY": str(parent), "RUST_LOG": "clarion=info"}
        run([str(args.clarion_bin), "validate", "--config", str(config)], nested_env)
        core_log = (artifacts / "compositor.log").open("w")
        logs.append(core_log)
        core = subprocess.Popen([str(args.clarion_bin), "--config", str(config), "--renderer", "gles"], env=nested_env,
                                stdout=core_log, stderr=subprocess.STDOUT, start_new_session=True)
        display = wait_for("private Wayland display", lambda: next((p for p in runtime.glob("wayland-*") if p.is_socket()), None), core)
        env["WAYLAND_DISPLAY"] = str(display)
        portal_log = (artifacts / "fake-portal.log").open("w")
        logs.append(portal_log)
        portal = subprocess.Popen([str(args.probe_bin), "--serve-portal"], env=env,
                                  stdout=portal_log, stderr=subprocess.STDOUT, start_new_session=True)
        wait_for("private fake FileChooser", lambda: (private / "portal-ready").is_file(), portal)
        report["probes"] = {}
        for index, kind in enumerate(("layer", "toplevel")):
            log = (artifacts / f"{kind}-probe.log").open("w")
            logs.append(log)
            probe = subprocess.Popen([str(args.probe_bin), f"--{kind}"], env=env | {"WAYLAND_DEBUG": "client"},
                                     stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            require(probe.wait(timeout=args.timeout) == 0, f"{kind} probe failed; inspect {log.name}")
            probe = None
            focused = private / f"{kind}-focused"
            require(focused.is_file(), f"{kind} did not confirm real platform keyboard focus")
            evidence = json.loads((private / f"{kind}-probe.json").read_text())
            require(evidence.get("passed") is True and evidence.get("focusedBefore") is True
                    and evidence.get("focusedAfter") is True and evidence.get("cancelled") is True, f"bad {kind} evidence")
            records = [json.loads(line) for line in (private / "portal-requests.jsonl").read_text().splitlines()]
            require(len(records) == index + 1, f"expected exactly one real FileChooser request for {kind}")
            identifier = records[-1]["parent"]
            protocol = Path(log.name).read_text(errors="replace")
            exports = protocol.count(".export_toplevel(")
            if kind == "layer":
                require(identifier == "" and exports == 0, "layer-shell must pass an empty parent without an invalid xdg export")
            else:
                require(identifier.startswith("wayland:") and len(identifier) > len("wayland:"), "ordinary xdg_toplevel must retain its exported portal parent")
                require(exports > 0, "ordinary probe did not issue a real export_toplevel")
            evidence.update(parent=identifier, exportRequests=exports)
            report["probes"][kind] = evidence
            (artifacts / f"{kind}-probe.json").write_text(json.dumps(evidence, indent=2) + "\n")
        require(core.poll() is None and portal.poll() is None, "a private server exited unexpectedly")
        core_text = (artifacts / "compositor.log").read_text(errors="replace")
        require("exported surface had an invalid role" not in core_text and "Protocol error" not in core_text, "private compositor reported a protocol failure")
        report["passed"] = True
    except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
        print(str(error), file=sys.stderr)
    finally:
        stop(probe)
        stop(portal)
        stop(core)
        for log in logs:
            log.close()
        report["cleanup"] = True
        (artifacts / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"Evidence: {artifacts / 'result.json'}", flush=True)
    return 0 if report["passed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--probe-bin", type=Path, default=ROOT / "target/debug/examples/portal_parent_probe")
    parser.add_argument("--clarion-bin", type=Path, default=ROOT / "target/debug/clarion")
    parser.add_argument("--timeout", type=int, default=60)
    parser.add_argument("--private-session", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--private-root", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.artifacts = args.artifacts.resolve()
    args.clarion_bin = args.clarion_bin.resolve(strict=True)
    args.probe_bin = args.probe_bin.resolve(strict=True)
    if args.private_session:
        require(args.private_root is not None, "missing private root")
        return exercise(args)
    require(shutil.which("dbus-run-session"), "missing dbus-run-session")
    parent = Path(os.environ.get("WAYLAND_DISPLAY", ""))
    require(str(parent) != ".", "a parent Wayland display is required")
    if not parent.is_absolute():
        parent = Path(os.environ["XDG_RUNTIME_DIR"]) / parent
    require(parent.is_socket(), "parent display is unavailable")
    with tempfile.TemporaryDirectory(prefix="clarion-portal-parent-") as temporary:
        private = Path(temporary).resolve()
        env = _driver.private_env(private)
        (private / "home").mkdir(mode=0o700)
        env.update(HOME=str(private / "home"), CLARION_PORTAL_ISOLATED_TEST="1", CLARION_PORTAL_TEST_ROOT=str(private),
                   CLARION_PORTAL_PARENT_WAYLAND=str(parent.resolve()))
        (private / ".portal-test-owner").write_text(str(private))
        config = private / "bus.conf"
        config.write_text('<busconfig><type>session</type><listen>unix:path=' + escape(str(private / 'runtime/bus')) +
                          '</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/>'
                          '<allow receive_sender="*"/><allow own="*"/></policy></busconfig>\n')
        command = ["dbus-run-session", "--config-file=" + str(config), "--", sys.executable, str(Path(__file__).resolve()),
                   "--private-session", "--private-root", str(private), "--artifacts", str(args.artifacts),
                   "--clarion-bin", str(args.clarion_bin), "--probe-bin", str(args.probe_bin), "--timeout", str(args.timeout)]
        child = subprocess.Popen(command, env=env, start_new_session=True)
        try:
            return child.wait()
        finally:
            stop(child)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Portal parent test: {error}", file=sys.stderr)
        raise SystemExit(1)
