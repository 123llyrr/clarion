#!/usr/bin/env python3
"""Exercise the native GPUI Shell only on an owned nested Clarion and private bus.

Only the disposable compositor receives the parent Wayland address. The Shell,
IPC commands and optional screenshots are confined to a new runtime directory.
This never starts/stops a systemd service or reads the parent clipboard.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[1]
MARKER = ".native-shell-smoke-owner"


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def stop(process):
    if process is None or process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=3)


def wait_for(description, predicate, processes=(), timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for process in processes:
            require(process.poll() is None,
                    f"{description}: process {process.pid} exited with {process.returncode}")
        value = predicate()
        if value:
            return value
        time.sleep(0.06)
    raise RuntimeError(f"Timed out: {description}")


def run(command, env, timeout=10):
    result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=timeout)
    require(result.returncode == 0, f"Command failed: {command!r}\n{result.stderr[-3000:]}")
    return result.stdout


def isolated_environment(private):
    env = os.environ.copy()
    for variable in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "WAYLAND_DEBUG", "CLARION_SOCKET",
                     "REALAND_SOCKET", "CLARION_CONFIG", "REALAND_CONFIG", "CLARION_SHELL_SOCKET",
                     "REALAND_SHELL_SOCKET", "DBUS_SESSION_BUS_ADDRESS", "DBUS_STARTER_ADDRESS",
                     "DBUS_STARTER_BUS_TYPE", "NOTIFY_SOCKET", "WATCHDOG_PID", "WATCHDOG_USEC",
                     "SESSION_MANAGER", "GTK_MODULES", "GDK_PIXBUF_MODULE_FILE"):
        env.pop(variable, None)
    for variable, directory in (("HOME", "home"), ("XDG_RUNTIME_DIR", "runtime"),
                                ("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data"),
                                ("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache"),
                                ("XDG_DATA_DIRS", "system-data")):
        path = private / directory
        path.mkdir(mode=0o700)
        env[variable] = str(path)
    # Restrict the app catalog without hiding Vulkan driver discovery.
    if not env.get("VK_DRIVER_FILES") and not env.get("VK_ICD_FILENAMES"):
        drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("*.json"))
        drivers += sorted(Path("/etc/vulkan/icd.d").glob("*.json"))
        if drivers:
            env["VK_DRIVER_FILES"] = os.pathsep.join(map(str, drivers))
    env.update(XDG_SESSION_TYPE="wayland", XDG_CURRENT_DESKTOP="clarion",
               WINIT_UNIX_BACKEND="wayland", GPUI_PLATFORM="wayland",
               CLARION_DISABLE_SYSTEM_MANAGER_NOTIFY="1", CLARION_SHELL_ISOLATED_TEST="1")
    return env


def guarded_worker_environment(private):
    private = private.resolve(strict=True)
    env = os.environ.copy()
    require(env.get("CLARION_NATIVE_SMOKE_ROOT") == str(private), "Missing owned test root")
    require((private / MARKER).read_text() == env.get("CLARION_NATIVE_SMOKE_TOKEN"),
            "Invalid owned test root marker")
    runtime = Path(env.get("XDG_RUNTIME_DIR", "")).resolve(strict=True)
    require(runtime == private / "runtime" and runtime.stat().st_uid == os.getuid()
            and stat.S_IMODE(runtime.stat().st_mode) == 0o700, "Unsafe test runtime")
    require(env.get("DBUS_SESSION_BUS_ADDRESS", "").startswith(f"unix:path={runtime}/bus"),
            "D-Bus is not confined to the owned runtime")
    for variable, directory in (("HOME", "home"), ("XDG_CONFIG_HOME", "config"),
                                ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"),
                                ("XDG_CACHE_HOME", "cache"), ("XDG_DATA_DIRS", "system-data")):
        require(env.get(variable) == str(private / directory), f"Unsafe {variable}")
    parent = Path(env.get("CLARION_NATIVE_SMOKE_PARENT", ""))
    require(parent.is_absolute() and parent.is_socket() and not parent.is_relative_to(private),
            "Parent Wayland address is not an external socket")
    require(not env.get("CLARION_SOCKET") and not env.get("CLARION_SHELL_SOCKET"),
            "Worker inherited a desktop IPC address")
    return env, runtime, parent


def seed_catalog(private, env):
    """Use only disposable entries; none of their Exec commands are invoked."""
    applications = Path(env["XDG_DATA_HOME"]) / "applications"
    applications.mkdir(parents=True)
    icons = private / "fixture-icons"
    icons.mkdir()
    identifiers = []
    colors = ("#4f8ff0", "#b16ae5", "#56ad7e", "#db9a42", "#db6876")
    for index in range(20):
        identifier = f"org.clarion.SmokeApp{index:02}.desktop"
        icon = icons / f"app-{index:02}.svg"
        icon.write_text('<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 128 128">'
                        f'<rect x="4" y="4" width="120" height="120" rx="28" fill="{colors[index % len(colors)]}"/>'
                        f'<circle cx="64" cy="64" r="{18 + index % 12}" fill="white" fill-opacity=".9"/></svg>')
        (applications / identifier).write_text(
            f'[Desktop Entry]\nType=Application\nName=测试应用 {index + 1:02}\n'
            f'Name[en]=Fixture App {index + 1:02}\nIcon={icon}\nExec=/usr/bin/true\n'
            f'Categories={"Utility" if index % 2 else "Graphics"};\nTerminal=false\n')
        identifiers.append(identifier)
    preferences = Path(env["XDG_CONFIG_HOME"]) / "clarion-shell"
    preferences.mkdir(parents=True)
    (preferences / "preferences.json").write_text(json.dumps(
        {"theme": "dark", "pinned_app_ids": identifiers[:6], "recent_app_ids": identifiers[6:9]},
        ensure_ascii=False) + "\n")
    desktop = Path(env["HOME"]) / "Desktop"
    desktop.mkdir()
    (desktop / "测试文档.txt").write_text("Owned native Shell fixture.\n")
    (Path(env["XDG_CONFIG_HOME"]) / "user-dirs.dirs").write_text(f'XDG_DESKTOP_DIR="{desktop}"\n')
    return identifiers


def require_no_legacy_launcher(roots):
    rows = subprocess.run(["ps", "-eo", "pid=,ppid="], capture_output=True, text=True,
                          check=True, timeout=3).stdout.splitlines()
    parents = {int(row.split()[0]): int(row.split()[1]) for row in rows if len(row.split()) == 2}
    descendants = set(roots)
    while True:
        more = {pid for pid, parent in parents.items() if parent in descendants} - descendants
        if not more:
            break
        descendants.update(more)
    for pid in descendants:
        try:
            executable = Path(f"/proc/{pid}/exe").resolve(strict=True).name
        except OSError:
            continue
        require(executable != "clarion-launcher", "Owned Shell spawned a retired standalone launcher")


def protocol_evidence(trace):
    effect_get = re.findall(r"ext_background_effect_manager_v1[@#]\d+\.get_background_effect\(", trace)
    effect_set = re.findall(r"ext_background_effect_surface_v1[@#]\d+\.set_blur_region\(wl_region[@#]\d+\)", trace)
    identity = re.findall(
        r'gtk_surface1[@#]\d+\.set_dbus_properties\("io\.github\.clarion\.Desktop\.MacOS",([^\n)]*)\)', trace)
    require(effect_get and effect_set, "Native trace did not request ext-background-effect regions")
    require(identity, "Desktop identity was absent or exported non-nil menu/object paths")
    for properties in identity:
        fields = [field.strip() for field in properties.split(",")]
        require(len(fields) == 5 and all(field in {"nil", "null"} for field in fields),
                "Desktop identity exported non-nil menu/object paths")
    return {"background_effect_objects": len(effect_get), "blur_region_requests": len(effect_set),
            "metadata_only_desktop_identities": len(identity), "trace_scope": "owned nested Shell only"}


def probe_ipc(path):
    results = []
    for request in ("Version", "Outputs"):
        for half_close in (False, True):
            started = time.monotonic()
            item = {"request": request, "half_close": half_close, "ok": False}
            try:
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                    stream.settimeout(1.5)
                    stream.connect(str(path))
                    stream.sendall((json.dumps(request) + "\n").encode())
                    if half_close:
                        stream.shutdown(socket.SHUT_WR)
                    reply = bytearray()
                    while b"\n" not in reply:
                        chunk = stream.recv(4096)
                        require(chunk, "IPC closed before its reply")
                        reply.extend(chunk)
                        require(len(reply) <= 256 * 1024, "IPC probe reply exceeded its bound")
                    parsed = json.loads(reply.split(b"\n", 1)[0])
                    require(isinstance(parsed, dict) and "Ok" in parsed, "IPC probe returned an error")
                    item.update(ok=True, reply_bytes=len(reply))
            except (OSError, ValueError, RuntimeError) as error:
                item["error"] = str(error)
            item["elapsed_ms"] = round((time.monotonic() - started) * 1000, 2)
            results.append(item)
    return results


def capture_owned_core(core, artifacts, env):
    tasks = {}
    for task in Path(f"/proc/{core.pid}/task").glob("*"):
        try:
            tasks[task.name] = {"name": (task / "comm").read_text().strip(),
                                "wchan": (task / "wchan").read_text().strip()}
        except OSError as error:
            tasks[task.name] = {"error": str(error)}
    (artifacts / "core-wchan.json").write_text(json.dumps(tasks, indent=2) + "\n")
    if not shutil.which("gdb"):
        return {"gdb": "unavailable", "thread_count": len(tasks)}
    started = time.monotonic()
    with (artifacts / "core-backtrace.log").open("w") as log:
        try:
            result = subprocess.run(["gdb", "-q", "-batch", "-p", str(core.pid),
                                     "-ex", "set pagination off", "-ex", "thread apply all bt",
                                     "-ex", "detach"], env=env, stdout=log,
                                    stderr=subprocess.STDOUT, timeout=8)
            return {"gdb_exit": result.returncode, "thread_count": len(tasks),
                    "seconds": round(time.monotonic() - started, 3), "debugger_may_pause_core": True}
        except subprocess.TimeoutExpired:
            return {"gdb": "timed out", "thread_count": len(tasks), "debugger_may_pause_core": True}


def exercise(args):
    env, runtime, parent = guarded_worker_environment(args.worker_root)
    private = args.worker_root.resolve()
    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    report = {"passed": False, "toolkit": "gpui", "checks": {}, "cleanup": False,
              "scope": "one native Shell process on an owned winit output and private D-Bus",
              "parent_clipboard_access": False, "physical_hdr_or_hotplug_tested": False}
    processes = []
    logs = []
    notify = None
    def interrupted(number, _frame):
        raise RuntimeError(f"Interrupted by signal {number}; cleaning owned processes")

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    try:
        report["catalog_fixture_count"] = len(seed_catalog(private, env))
        config = private / "nested.kdl"
        config.write_text('input { keyboard { xkb { layout "us"; }; }; }\n'
                          'output "winit" { scale 1; }\nxwayland-satellite { off; }\n'
                          'hotkey-overlay { skip-at-startup; }\n')
        shutil.copyfile(config, artifacts / "nested.kdl")
        core_env = env | {"WAYLAND_DISPLAY": str(parent),
                          "RUST_LOG": os.environ.get("CLARION_NATIVE_SMOKE_RUST_LOG", "clarion=info")}
        run([str(args.clarion_bin), "validate", "--config", str(config)], core_env)

        def launch(name, command, environment):
            log = (artifacts / f"{name}.log").open("w")
            logs.append(log)
            process = subprocess.Popen(command, env=environment, stdout=log,
                                       stderr=subprocess.STDOUT, start_new_session=True)
            processes.append(process)
            return process

        core = launch("compositor", [str(args.clarion_bin), "--config", str(config),
                                    "--renderer", args.renderer], core_env)
        core_socket = wait_for("owned compositor IPC", lambda: next(
            (path for path in runtime.glob("clarion.*.sock") if path.is_socket()), None), (core,), args.timeout)
        display = wait_for("owned Wayland socket", lambda: next(
            (path for path in runtime.glob("wayland-*") if path.is_socket()), None), (core,), args.timeout)
        require(display != parent and display.resolve().parent == runtime, "Unsafe nested display")
        env.update(WAYLAND_DISPLAY=str(display), CLARION_SOCKET=str(core_socket),
                   CLARION_SHELL_SOCKET=str(runtime / "clarion-shell.sock"))

        def ipc(command):
            return json.loads(run([str(args.clarion_bin), "msg", "-j", command], env))

        outputs = wait_for("winit output", lambda: ipc("outputs"), (core,), args.timeout)
        require(set(outputs) == {"winit"}, f"Unexpected non-nested outputs: {list(outputs)}")
        report["ipc_probe"] = {"initial": probe_ipc(core_socket)}
        (artifacts / "ipc-probe.json").write_text(json.dumps(report["ipc_probe"], indent=2) + "\n")
        if args.renderer == "vulkan":
            require("nested native Vulkan WSI renderer initialized" in (artifacts / "compositor.log").read_text(),
                    "Nested Vulkan was requested but did not initialize; refusing a fallback pass")
        report["renderer"] = args.renderer
        require(not ipc("layers"), "Owned compositor unexpectedly already has Shell layers")
        notify_path = runtime / "shell-ready"
        notify = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        notify.bind(str(notify_path))
        notify.setblocking(False)
        shell_env = env | {"NOTIFY_SOCKET": str(notify_path), "WAYLAND_DEBUG": "client"}
        shell_command = [str(args.shell_bin), "--monitor=0", "--output-name=winit",
                         f"--quit-after={min(3600, args.timeout * 12 + 90)}"]
        first_frame_started = time.monotonic()
        shell = launch("shell", shell_command, shell_env)
        shell_socket = runtime / "clarion-shell.sock"
        wait_for("native Shell command socket", shell_socket.is_socket, (core, shell), args.timeout)

        def ready():
            try:
                return b"READY=1" in notify.recv(8192).splitlines()
            except BlockingIOError:
                return False

        wait_for("native first frame READY", ready, (core, shell), args.timeout)
        report["first_frame_seconds"] = round(time.monotonic() - first_frame_started, 3)
        bar = wait_for("native bar layer", lambda: next(
            (layer for layer in ipc("layers") if layer["namespace"] == "clarion-shell"), None),
            (core, shell), args.timeout)
        require(bar["output"] == "winit" and bar["layer"] == "Top"
                and bar["keyboard_interactivity"] == "None", f"Unexpected bar layer: {bar}")
        report["checks"]["first_frame_ready"] = True
        report["checks"]["bar"] = bar
        report["ipc_probe"]["active"] = probe_ipc(core_socket)
        (artifacts / "ipc-probe.json").write_text(json.dumps(report["ipc_probe"], indent=2) + "\n")
        if args.diagnose_ipc:
            report.update(diagnostic_only=True, diagnostic_complete=False)
            if shutil.which("strace"):
                trace = launch("strace", ["strace", "-f", "-tt", "-T", "-s", "256",
                                          "-e", "trace=network,poll,ppoll", "-p", str(shell.pid),
                                          "-o", str(artifacts / "shell-ipc.strace")], env)
                time.sleep(3)
                report["ipc_probe"]["during_strace"] = probe_ipc(core_socket)
                stop(trace)
                report["strace_exit"] = trace.returncode
            else:
                report["strace"] = "unavailable"
            # Probe before attaching a debugger, since ptrace itself pauses the
            # compositor and must not contaminate the baseline latency samples.
            report["core_debugger"] = capture_owned_core(core, artifacts, env)
            (artifacts / "ipc-probe.json").write_text(json.dumps(report["ipc_probe"], indent=2) + "\n")
            report["diagnostic_complete"] = True
            return

        def panel():
            return [layer for layer in ipc("layers") if layer["namespace"] == "clarion-panel"]

        def send(command):
            run([sys.executable, str(ROOT / "resources/clarion-shell-msg"), command], env)

        report["panel_timings"] = {}
        for command in ("launcher", "control-center", "notifications", "screenshot", "preferences", "recent-content"):
            started = time.monotonic()
            send(command)
            observed = []
            def configured_panel():
                nonlocal observed
                observed = panel()
                return observed if (len(observed) == 1 and observed[0]["output"] == "winit"
                                    and observed[0]["layer"] == "Overlay"
                                    and observed[0]["keyboard_interactivity"] == "Exclusive") else None
            try:
                layers = wait_for(f"{command} configured panel", configured_panel,
                                  (core, shell), args.timeout)
            except RuntimeError as error:
                raise RuntimeError(f"{error}; last observed panel roles: {observed}") from error
            timing = {"configured_seconds": round(time.monotonic() - started, 3)}
            if args.capture:
                started = time.monotonic()
                run(["grim", "-o", "winit", str(artifacts / f"{command}.png")], env)
                timing["capture_seconds"] = round(time.monotonic() - started, 3)
            started = time.monotonic()
            send("close")
            wait_for(f"{command} closed", lambda: not panel(), (core, shell), args.timeout)
            timing["closed_seconds"] = round(time.monotonic() - started, 3)
            report["panel_timings"][command] = timing
            report["checks"][command] = True
            require_no_legacy_launcher({core.pid, shell.pid})
        require(not (runtime / "clarion-launcher.sock").exists(),
                "Apps unexpectedly spawned the retired standalone launcher")
        report["checks"]["integrated_apps"] = True
        report["checks"]["no_standalone_launcher_process"] = True
        report["protocol"] = protocol_evidence((artifacts / "shell.log").read_text())
        report["ipc_probe"]["after_panels"] = probe_ipc(core_socket)
        (artifacts / "ipc-probe.json").write_text(json.dumps(report["ipc_probe"], indent=2) + "\n")
        inode = shell_socket.stat().st_ino
        duplicate = launch("duplicate", shell_command, env)
        try:
            duplicate.wait(timeout=min(5, args.timeout))
        except subprocess.TimeoutExpired as error:
            raise RuntimeError("Duplicate Shell was not refused") from error
        require(duplicate.returncode != 0 and shell.poll() is None
                and shell_socket.stat().st_ino == inode, "Duplicate replaced the active owner")
        report["checks"]["duplicate_preserves_owner"] = True
        preview_started = time.monotonic()
        preview = launch("preview", [str(args.shell_bin), "--preview", "--quit-after=2"], env)
        # Font initialization occurs before GPUI's event loop and its diagnostic
        # quit timer. Keep the production READY deadline above strict; allow a
        # separate cold-font budget for this additional preview process.
        require(preview.wait(timeout=max(60, args.timeout)) == 0, "Native preview failed")
        report["preview_seconds"] = round(time.monotonic() - preview_started, 3)
        require(shell_socket.stat().st_ino == inode and shell.poll() is None,
                "Preview changed the real command endpoint")
        report["checks"]["preview_preserves_owner"] = True
        timely = all(item["ok"] and item["elapsed_ms"] < 900
                     for phase in report["ipc_probe"].values() for item in phase)
        report["checks"]["ipc_reply_within_backend_deadline"] = timely
        require(timely, "Owned compositor IPC exceeded the Shell backend deadline; inspect ipc-probe.json")
        report["passed"] = True
    except BaseException as error:
        report["error"] = str(error)
        raise
    finally:
        for process in reversed(processes):
            stop(process)
        if notify is not None:
            notify.close()
        for log in logs:
            log.close()
        report["cleanup"] = all(process.poll() is not None for process in processes)
        (artifacts / "report.json").write_text(json.dumps(report, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clarion-bin", type=Path, default=ROOT / "target/debug/clarion")
    parser.add_argument("--shell-bin", type=Path, default=ROOT / "target/debug/clarion-shell")
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--renderer", choices=("vulkan", "gles"), default="gles")
    parser.add_argument("--timeout", type=int, default=20)
    parser.add_argument("--capture", action="store_true", help="Capture owned nested panels with grim")
    parser.add_argument("--diagnose-ipc", action="store_true",
                        help="Stop after READY and private IPC/thread/syscall diagnostics; no full acceptance")
    parser.add_argument("--worker-root", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    require(3 <= args.timeout <= 120, "Timeout must be between 3 and 120 seconds")
    for attribute in ("clarion_bin", "shell_bin"):
        path = getattr(args, attribute).resolve(strict=True)
        require(path.is_file() and os.access(path, os.X_OK), f"Not an executable: {path}")
        setattr(args, attribute, path)
    args.artifacts = args.artifacts.resolve()
    if args.worker_root is not None:
        exercise(args)
        return 0
    require(shutil.which("dbus-run-session"), "dbus-run-session is required")
    if args.capture:
        require(shutil.which("grim"), "--capture requires grim")
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", ""))
    display = os.environ.get("WAYLAND_DISPLAY", "")
    require(display and runtime.is_absolute(), "Run from an existing Wayland desktop")
    parent = (Path(display) if display.startswith("/") else runtime / display).resolve(strict=True)
    require(parent.is_socket(), "Parent Wayland address is not a socket")
    with tempfile.TemporaryDirectory(prefix="clarion-native-shell-") as directory:
        private = Path(directory).resolve()
        token = secrets.token_hex(24)
        (private / MARKER).write_text(token)
        env = isolated_environment(private)
        env.update(CLARION_NATIVE_SMOKE_ROOT=str(private), CLARION_NATIVE_SMOKE_TOKEN=token,
                   CLARION_NATIVE_SMOKE_PARENT=str(parent))
        config = private / "bus.conf"
        config.write_text('<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" '
                          '"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">'
                          '<busconfig><type>session</type><listen>unix:path='
                          + escape(str(private / "runtime/bus"))
                          + '</listen><auth>EXTERNAL</auth><policy context="default">'
                          '<allow send_destination="*"/><allow receive_sender="*"/>'
                          '<allow own="*"/></policy></busconfig>')
        command = ["dbus-run-session", f"--config-file={config}", "--", sys.executable,
                   str(Path(__file__).resolve()), "--worker-root", str(private),
                   "--clarion-bin", str(args.clarion_bin), "--shell-bin", str(args.shell_bin),
                   "--artifacts", str(args.artifacts), "--renderer", args.renderer,
                   "--timeout", str(args.timeout)]
        if args.capture:
            command.append("--capture")
        if args.diagnose_ipc:
            command.append("--diagnose-ipc")
        child = subprocess.Popen(command, env=env, start_new_session=True)
        try:
            return child.wait(timeout=args.timeout * 12 + 90)
        finally:
            stop(child)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Native Shell smoke: {error}", file=sys.stderr)
        raise SystemExit(1)
