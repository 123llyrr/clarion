#!/usr/bin/env python3
"""Real clipboard integration on an owned nested Clarion and private D-Bus.

The parent Wayland socket is given only to the disposable compositor. Every
producer, consumer and Flutter process is guarded against the private socket.
No command reads or changes the parent session's clipboard.
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


def wait_for(description, ready, process=None, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process is not None and process.poll() is not None:
            raise RuntimeError(f"{description}: process exited with {process.returncode}")
        result = ready()
        if result:
            return result
        time.sleep(0.08)
    raise RuntimeError(f"Timed out: {description}")


def run(command, env, timeout=20):
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
                                ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"),
                                ("XDG_CACHE_HOME", "cache")):
        path = private / directory
        path.mkdir(mode=0o700, exist_ok=True)
        env[variable] = str(path)
    env.update(CLARION_CLIPBOARD_TEST_ROOT=str(private), CLARION_CLIPBOARD_ISOLATED_TEST="1",
               CLARION_SHELL_ISOLATED_TEST="1", CLARION_SHELL_DISABLE_DESKTOP="1",
               CLARION_DISABLE_SYSTEM_MANAGER_NOTIFY="1", GDK_BACKEND="wayland",
               WINIT_UNIX_BACKEND="wayland", XDG_SESSION_TYPE="wayland", XDG_CURRENT_DESKTOP="clarion")
    return env


def exercise(args):
    private = args.private_root.resolve()
    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    for name in ("native-evidence.json", "preview-evidence.json", "native-panel.png"):
        (artifacts / name).unlink(missing_ok=True)
    env = os.environ.copy()
    runtime = Path(env["XDG_RUNTIME_DIR"])
    require(runtime.parent == private and stat.S_IMODE(runtime.stat().st_mode) == 0o700, "unsafe private runtime")
    require(env.get("DBUS_SESSION_BUS_ADDRESS", "").startswith(f"unix:path={runtime}/"), "D-Bus address is not private")
    parent = Path(env["CLARION_CLIPBOARD_PARENT_WAYLAND"])
    require(parent.is_absolute() and parent.is_socket() and parent.parent != runtime, "invalid parent display")
    (runtime / "clipboard-test-owner").write_text(str(private))
    report = {"passed": False, "isolation": {"privateRuntimeMode": "0700", "privateBus": True,
              "clipboardScope": "owned nested compositor only", "parentClipboardAccess": False}, "cleanup": False}
    compositor = flutter = preview = None
    logs = []
    try:
        for tool in ("cc", "pkg-config", "wayland-scanner"):
            require(shutil.which(tool), f"Missing dependency: {tool}")
        require(args.clarion_bin.is_file(), "Build the compositor first")
        require(Path(args.flutter).is_file(), "Flutter SDK was not found")
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
        ipc = wait_for("private IPC", lambda: next(runtime.glob("clarion.*.sock"), None), compositor)
        require(display.parent == runtime and display != parent and ipc.parent == runtime, "test escaped its private sockets")
        env.update(WAYLAND_DISPLAY=str(display), CLARION_SOCKET=str(ipc),
                   CLARION_CLIPBOARD_ARTIFACTS=str(artifacts), CLARION_CLIPBOARD_CLARION_BIN=str(args.clarion_bin))
        generated = private / "ext-data-control-v1-protocol.c"
        header = private / "ext-data-control-v1-client-protocol.h"
        xml = ROOT / "clarion-shell/linux/runner/protocols/ext-data-control-v1.xml"
        run(["wayland-scanner", "client-header", str(xml), str(header)], env)
        run(["wayland-scanner", "private-code", str(xml), str(generated)], env)
        flags = shlex.split(run(["pkg-config", "--cflags", "--libs", "wayland-client"], env))
        peer = private / "clipboard-peer"
        run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", str(ROOT / "scripts/fixtures/clipboard_peer.c"),
             str(generated), "-I", str(private), "-o", str(peer), *flags], env)
        env["CLARION_CLIPBOARD_PEER"] = str(peer)
        # Even D-Bus activation sees no parent display. The private bus has no
        # service directories, so portals/clipboard tools cannot escape it.
        if shutil.which("dbus-update-activation-environment"):
            run(["dbus-update-activation-environment", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR",
                 "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME"], env)
        test_log = (artifacts / "flutter-native.log").open("w")
        logs.append(test_log)
        command = [args.flutter, "test", "--no-pub", "-d", "linux", "integration_test/recent_content_native_test.dart", "--reporter=expanded"]
        print("Running real clipboard integration on private sockets.", flush=True)
        flutter = subprocess.Popen(command, cwd=ROOT / "clarion-shell", env=env, stdout=test_log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        require(flutter.wait(timeout=args.timeout) == 0, "Native Flutter integration failed; inspect flutter-native.log")
        flutter = None
        evidence = artifacts / "native-evidence.json"
        require(evidence.is_file(), "Native test did not write its evidence")
        report["native"] = json.loads(evidence.read_text())
        require(report["native"].get("passed") is True, "Native evidence failed")
        build_log = (artifacts / "flutter-preview-build.log").open("w")
        logs.append(build_log)
        preview_build = [args.flutter, "build", "linux", "--debug", "--no-pub", "--target",
                         "integration_test/fixtures/recent_content_preview_probe.dart"]
        flutter = subprocess.Popen(preview_build, cwd=ROOT / "clarion-shell", env=env, stdout=build_log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        require(flutter.wait(timeout=args.timeout) == 0, "Preview probe build failed; inspect flutter-preview-build.log")
        flutter = None
        bundle = ROOT / "clarion-shell/build/linux/x64/debug/bundle/clarion-shell"
        require(bundle.is_file(), f"Missing compiled integration runner: {bundle}")
        preview_log = (artifacts / "flutter-preview.log").open("w")
        logs.append(preview_log)
        print("Checking preview cannot capture or publish clipboard contents.", flush=True)
        preview = subprocess.Popen([str(bundle), "--preview", "--no-desktop"], cwd=bundle.parent,
                                   env=env | {"CLARION_CLIPBOARD_PREVIEW_TEST": "1"}, stdout=preview_log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        preview_evidence = artifacts / "preview-evidence.json"
        wait_for("preview isolation evidence", preview_evidence.is_file, preview, timeout=60)
        report["preview"] = json.loads(preview_evidence.read_text())
        require(report["preview"].get("passed") is True, "Preview evidence failed")
        report["passed"] = True
    except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
        print(f"Native clipboard integration failed: {error}", file=sys.stderr)
    finally:
        stop(preview)
        stop(flutter)
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
    parser.add_argument("--clarion-bin", type=Path, default=ROOT / "target/debug/clarion")
    candidates = [Path.home() / ".cache/clarion/toolchains/flutter/bin/flutter",
                  Path.home() / ".cache/realand/toolchains/flutter/bin/flutter"]
    default_flutter = os.environ.get("CLARION_FLUTTER_BIN") or next((str(path) for path in candidates if path.is_file()), "flutter")
    parser.add_argument("--flutter", default=default_flutter)
    parser.add_argument("--timeout", type=int, default=360)
    parser.add_argument("--private-session", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--private-root", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.clarion_bin = args.clarion_bin.resolve()
    args.artifacts = args.artifacts.resolve()
    if args.private_session:
        require(args.private_root is not None, "Missing owned private root")
        return exercise(args)
    require(shutil.which("dbus-run-session"), "Missing dbus-run-session")
    parent = Path(os.environ.get("WAYLAND_DISPLAY", ""))
    require(str(parent) != ".", "A parent Wayland display is required")
    if not parent.is_absolute():
        parent = Path(os.environ["XDG_RUNTIME_DIR"]) / parent
    require(parent.is_socket(), "Parent Wayland socket is unavailable")
    with tempfile.TemporaryDirectory(prefix="clarion-clipboard-native-") as temporary:
        private = Path(temporary)
        env = private_env(private)
        env["CLARION_CLIPBOARD_PARENT_WAYLAND"] = str(parent.resolve())
        config = private / "bus.conf"
        config.write_text('<busconfig><type>session</type><listen>unix:path=' + escape(str(private / 'runtime/bus')) +
                          '</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/><allow receive_sender="*"/>'
                          '<allow own="*"/></policy></busconfig>\n')
        command = ["dbus-run-session", "--config-file=" + str(config), "--", sys.executable, str(Path(__file__).resolve()),
                   "--private-session", "--private-root", str(private), "--artifacts", str(args.artifacts),
                   "--clarion-bin", str(args.clarion_bin), "--flutter", args.flutter, "--timeout", str(args.timeout)]
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
