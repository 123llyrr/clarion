#!/usr/bin/env python3
"""Exercise native Rust Shell services only on disposable Wayland/D-Bus sockets.

Build with cargo test -p clarion-shell --offline -j2 --no-run, then pass the
reported target/debug/deps/clarion_shell-... executable as --test-bin. The
parent Wayland socket is used solely as the nested compositor's display.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[1]


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def stop(process):
    if process is None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)


def wait_for(description, ready, process, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        require(process.poll() is None, f"{description}: process exited with {process.returncode}")
        value = ready()
        if value:
            return value
        time.sleep(0.05)
    raise RuntimeError(f"Timed out: {description}")


def run(command, env, timeout=30):
    result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=timeout)
    require(result.returncode == 0, f"Command failed {command!r}: {result.stderr[-3000:]}")
    return result.stdout


def private_env(private):
    env = os.environ.copy()
    for name in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "WAYLAND_DEBUG", "CLARION_SOCKET",
                 "REALAND_SOCKET", "CLARION_CONFIG", "REALAND_CONFIG", "NOTIFY_SOCKET", "WATCHDOG_PID",
                 "WATCHDOG_USEC", "DBUS_SESSION_BUS_ADDRESS", "DBUS_STARTER_ADDRESS", "DBUS_STARTER_BUS_TYPE"):
        env.pop(name, None)
    for variable, directory in (("XDG_RUNTIME_DIR", "runtime"), ("XDG_CONFIG_HOME", "config"),
                                ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache")):
        path = private / directory
        path.mkdir(mode=0o700)
        env[variable] = str(path)
    env.update(CLARION_CLIPBOARD_ISOLATED_TEST="1", CLARION_CLIPBOARD_TEST_ROOT=str(private),
               CLARION_SHELL_ISOLATED_TEST="1", CLARION_SHELL_DISABLE_DESKTOP="1",
               CLARION_DISABLE_SYSTEM_MANAGER_NOTIFY="1", WINIT_UNIX_BACKEND="wayland",
               XDG_SESSION_TYPE="wayland", XDG_CURRENT_DESKTOP="clarion")
    return env


def exercise(args):
    private = args.private_root.resolve()
    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    runtime = Path(env["XDG_RUNTIME_DIR"])
    require(runtime.parent == private and stat.S_IMODE(runtime.stat().st_mode) == 0o700, "unsafe private runtime")
    require(env.get("DBUS_SESSION_BUS_ADDRESS", "").startswith(f"unix:path={runtime}/"), "D-Bus is not private")
    parent = Path(env["CLARION_CLIPBOARD_PARENT_WAYLAND"])
    require(parent.is_absolute() and parent.is_socket() and parent.parent != runtime, "invalid parent display")
    (runtime / "clipboard-test-owner").write_text(str(private))
    evidence = artifacts / "native-services-evidence.json"
    evidence.unlink(missing_ok=True)
    report = {"passed": False, "isolation": {"runtimeMode": "0700", "privateBus": True,
              "clipboardScope": "owned nested compositor", "parentClipboardAccess": False}, "cleanup": False}
    compositor = test = None
    logs = []
    try:
        for name in ("cc", "pkg-config", "wayland-scanner"):
            require(shutil.which(name), f"Missing dependency: {name}")
        require(args.test_bin.is_file() and os.access(args.test_bin, os.X_OK), "Missing Rust test executable")
        require(args.clarion_bin.is_file(), "Build Clarion first")
        # No spawn-at-startup directive and no existing user config are used.
        config = private / "nested.kdl"
        config.write_text('input { keyboard { xkb { layout "us"; }; }; }\noutput "winit" { scale 1; }\n'
                          'xwayland-satellite { off; }\nhotkey-overlay { skip-at-startup; }\n')
        shutil.copyfile(config, artifacts / "nested-config.kdl")
        nested_env = env | {"WAYLAND_DISPLAY": str(parent), "RUST_LOG": "clarion=info"}
        run([str(args.clarion_bin), "validate", "--config", str(config)], nested_env)
        core_log = (artifacts / "compositor.log").open("w")
        logs.append(core_log)
        compositor = subprocess.Popen([str(args.clarion_bin), "--config", str(config), "--renderer", "gles"],
                                      env=nested_env, stdout=core_log, stderr=subprocess.STDOUT, start_new_session=True)
        display = wait_for("private Wayland socket", lambda: next((p for p in runtime.glob("wayland-*") if p.is_socket()), None), compositor)
        ipc = wait_for("private IPC socket", lambda: next(runtime.glob("clarion.*.sock"), None), compositor)
        require(display.parent == runtime and display != parent and ipc.parent == runtime, "test escaped private sockets")
        env.update(WAYLAND_DISPLAY=str(display), CLARION_SOCKET=str(ipc), CLARION_NATIVE_SERVICE_ARTIFACTS=str(artifacts))
        generated = private / "ext-data-control-v1-protocol.c"
        header = private / "ext-data-control-v1-client-protocol.h"
        xml = ROOT / "scripts/fixtures/ext-data-control-v1.xml"
        run(["wayland-scanner", "client-header", str(xml), str(header)], env)
        run(["wayland-scanner", "private-code", str(xml), str(generated)], env)
        flags = shlex.split(run(["pkg-config", "--cflags", "--libs", "wayland-client"], env))
        peer = private / "clipboard-peer"
        run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", str(ROOT / "scripts/fixtures/clipboard_peer.c"),
             str(generated), "-I", str(private), "-o", str(peer), *flags], env)
        env["CLARION_CLIPBOARD_PEER"] = str(peer)
        for label, test_name, extra in (("preview", "preview_does_not_start_native_services", []),
                                        ("native", "isolated_native_protocols", ["--ignored"])):
            log = (artifacts / f"{label}-services.log").open("w")
            logs.append(log)
            command = [str(args.test_bin), f"services::native_tests::{test_name}", "--exact", "--nocapture", "--test-threads=1", *extra]
            print(f"Running {label} services test on private sockets", flush=True)
            test = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            require(test.wait(timeout=args.timeout) == 0, f"{label} Rust services test failed; inspect {log.name}")
            test = None
        require(evidence.is_file(), "Rust test did not write evidence")
        report["native"] = json.loads(evidence.read_text())
        require(report["native"].get("passed") is True, "Rust evidence failed")
        report["preview"] = {"passed": True, "noNativeThreadsOrConnections": True}
        report["passed"] = True
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
        print(str(error), file=sys.stderr)
    finally:
        stop(test)
        stop(compositor)
        for log in logs:
            log.close()
        report["cleanup"] = True
        (artifacts / "result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(f"Evidence: {artifacts / 'result.json'}", flush=True)
    return 0 if report["passed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--test-bin", type=Path, required=True)
    parser.add_argument("--clarion-bin", type=Path, default=ROOT / "target/debug/clarion")
    parser.add_argument("--timeout", type=int, default=120)
    parser.add_argument("--private-session", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--private-root", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.artifacts = args.artifacts.resolve()
    args.test_bin = args.test_bin.resolve()
    args.clarion_bin = args.clarion_bin.resolve()
    if args.private_session:
        require(args.private_root is not None, "missing private root")
        return exercise(args)
    require(shutil.which("dbus-run-session"), "Missing dbus-run-session")
    parent = Path(os.environ.get("WAYLAND_DISPLAY", ""))
    require(str(parent) != ".", "A parent Wayland display is required")
    if not parent.is_absolute():
        parent = Path(os.environ["XDG_RUNTIME_DIR"]) / parent
    require(parent.is_socket(), "Parent Wayland socket is unavailable")
    with tempfile.TemporaryDirectory(prefix="clarion-rust-services-") as temporary:
        private = Path(temporary)
        env = private_env(private)
        env["CLARION_CLIPBOARD_PARENT_WAYLAND"] = str(parent.resolve())
        config = private / "bus.conf"
        config.write_text('<busconfig><type>session</type><listen>unix:path=' + escape(str(private / 'runtime/bus')) +
                          '</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/><allow receive_sender="*"/>'
                          '<allow own="*"/></policy></busconfig>\n')
        command = ["dbus-run-session", "--config-file=" + str(config), "--", sys.executable, str(Path(__file__).resolve()),
                   "--private-session", "--private-root", str(private), "--artifacts", str(args.artifacts),
                   "--test-bin", str(args.test_bin), "--clarion-bin", str(args.clarion_bin), "--timeout", str(args.timeout)]
        child = subprocess.Popen(command, env=env, start_new_session=True)
        try:
            return child.wait()
        finally:
            stop(child)


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        raise SystemExit(main())
    except (RuntimeError, KeyboardInterrupt) as error:
        print(error, file=sys.stderr)
        raise SystemExit(1)
