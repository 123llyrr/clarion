#!/usr/bin/env python3
"""Native production Apps/IPC acceptance with isolated fixtures and a fake GIO.

No installed application can launch: the daemon sees only private desktop files,
and PATH resolves gio to a logging fixture. Input targets a disposable Clarion.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import select
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("launcher_spike_helpers", ROOT / "scripts/test-launcher-spike.py")
helpers = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helpers)
require, run, wait_for, stop, write_json = (helpers.require, helpers.run, helpers.wait_for,
                                         helpers.stop, helpers.write_json)


class Peer:
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(3)
        self.socket.connect(str(path))
        self.pending = b""
        self.events = []
        self.unread = []

    def send(self, kind, request_id, **fields):
        self.socket.sendall((json.dumps({"type": kind, "request_id": request_id, **fields},
                                       ensure_ascii=False) + "\n").encode())

    def pump(self, seconds=0.05):
        if not select.select([self.socket], [], [], seconds)[0]:
            return
        chunk = self.socket.recv(65_536)
        require(chunk, "Launcher IPC disconnected")
        self.pending += chunk
        while b"\n" in self.pending:
            line, self.pending = self.pending.split(b"\n", 1)
            event = json.loads(line)
            self.events.append(event)
            self.unread.append(event)

    def expect(self, kind, request_id, seconds=5):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            for index, event in enumerate(self.unread):
                if event["type"] == kind and event.get("request_id") == request_id:
                    return self.unread.pop(index)
                if event["type"] == "Error" and kind != "Error":
                    raise helpers.SmokeFailure(f"Unexpected launcher error: {event}")
            self.pump(min(0.1, max(0, deadline - time.monotonic())))
        raise helpers.SmokeFailure(f"Missing {kind} for request {request_id}; events={self.events[-8:]}")

    def settle(self, seconds=0.25):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self.pump(min(0.05, deadline - time.monotonic()))


def desktop(applications, filename, name, localized, keyword, category, icon):
    path = applications / filename
    path.write_text(f"[Desktop Entry]\nType=Application\nName={name}\nName[zh_CN]={localized}\n"
                    f"Comment[zh_CN]=隔离测试应用\nKeywords[zh_CN]={keyword};\n"
                    f"Categories={category};\nIcon={icon}\nExec=/bin/false\n")
    return path


def exercise(args):
    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    result = {"passed": False, "scope": "production GPUI Apps/IPC in isolated Clarion; fake GIO only",
              "checks": {}, "events": [], "cleanup": False,
              "capture_semantics": {
                  "method": "grim compositor readback",
                  "ime_and_f4": "inner compositor readback",
                  "host_wsi_presentation_asserted": False,
                  "note": "With native Vulkan WSI, readback contents do not establish continued host toplevel presentation."}}
    compositor = daemon = duplicate = guardian = shell_process = fullscreen = None
    inner_compositor = ime = ime_daemon = None
    orphan_pid = None
    launcher_socket = None
    peer = None
    logs = []
    private = tempfile.TemporaryDirectory(prefix="clarion-apps-native-")
    try:
        root = Path(private.name)
        env = os.environ.copy()
        parent_display = Path(env["CLARION_APPS_PARENT_WAYLAND"])
        require(env.get("DBUS_SESSION_BUS_ADDRESS") != env.get("CLARION_APPS_PARENT_DBUS"),
                "Production test must use a private D-Bus session")
        for variable in ("DISPLAY", "WAYLAND_SOCKET", "WAYLAND_DEBUG", "CLARION_SOCKET", "REALAND_SOCKET",
                         "CLARION_CONFIG", "REALAND_CONFIG", "NOTIFY_SOCKET", "WATCHDOG_PID", "WATCHDOG_USEC",
                         "CLARION_SHELL_SOCKET", "REALAND_SHELL_SOCKET", "SESSION_MANAGER"):
            env.pop(variable, None)
        for variable, directory in (("XDG_RUNTIME_DIR", "runtime"), ("XDG_CONFIG_HOME", "config"),
                                    ("XDG_DATA_HOME", "data"), ("XDG_CACHE_HOME", "cache"),
                                    ("XDG_STATE_HOME", "state"), ("XDG_DATA_DIRS", "system-data")):
            path = root / directory
            path.mkdir(mode=0o700)
            env[variable] = str(path)
        env.update(WAYLAND_DISPLAY=str(parent_display), WINIT_UNIX_BACKEND="wayland",
                   CLARION_DISABLE_SYSTEM_MANAGER_NOTIFY="1", XDG_CURRENT_DESKTOP="clarion",
                   XDG_SESSION_TYPE="wayland", LC_ALL="zh_CN.UTF-8", LANGUAGE="zh_CN",
                   RUST_LOG=os.environ.get("CLARION_APPS_RUST_LOG", "clarion=info"))
        # Catalog isolation must not hide Vulkan's driver JSON discovery, which
        # also consults XDG_DATA_DIRS. Preserve system ICDs explicitly while the
        # application scanner remains confined to private data directories.
        if not env.get("VK_DRIVER_FILES") and not env.get("VK_ICD_FILENAMES"):
            drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("*.json"))
            drivers += sorted(Path("/etc/vulkan/icd.d").glob("*.json"))
            require(drivers, "No Vulkan driver descriptors found for isolated rendering")
            env["VK_DRIVER_FILES"] = os.pathsep.join(map(str, drivers))
        runtime = Path(env["XDG_RUNTIME_DIR"])
        applications = Path(env["XDG_DATA_HOME"]) / "applications"
        applications.mkdir()
        icons = root / "fixture-icons"
        icons.mkdir()
        icon_paths = []
        for index, color in enumerate(("#468cff", "#e379d5", "#65c999", "#eead4f")):
            icon = icons / f"fixture-{index}.svg"
            icon.write_text('<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 128 128">'
                            f'<rect x="4" y="4" width="120" height="120" rx="27" fill="{color}"/>'
                            '<path d="M32 88L64 30L96 88Z" fill="white" fill-opacity=".9"/></svg>')
            icon_paths.append(icon)
        alpha = desktop(applications, "org.clarion.Alpha ${HOME} $(touch literal).desktop",
                        "Alpha Utility", "Alpha 工具", "uniquealpha;独特甲", "Utility", icon_paths[0])
        beta = desktop(applications, "org.clarion.Beta.desktop", "Beta Canvas", "Beta 创意画板",
                       "uniquebeta;画板", "Graphics", icon_paths[1])
        gamma = desktop(applications, "org.clarion.Gamma.desktop", "Gamma Chat", "Gamma 社交讯息",
                        "uniquegamma;聊天", "InstantMessaging", icon_paths[2])
        config_dir = Path(env["XDG_CONFIG_HOME"]) / "clarion-shell"
        config_dir.mkdir()
        preferences_path = config_dir / "preferences.json"
        preferences = {"theme": "dark", "accent": "#56a8ff", "recent_app_ids": [gamma.name], "reduce_motion": True}
        preferences_path.write_text(json.dumps(preferences) + "\n")
        original_preferences = preferences_path.read_bytes()
        fake_log = artifacts / "fake-gio.jsonl"
        fake_log.write_text("")
        failure_flag = root / "fail-gio"
        fake_bin = root / "bin"
        fake_bin.mkdir()
        fake_gio = fake_bin / "gio"
        fake_gio.write_text(f'#!{sys.executable}\n'
                            'import json,os,sys\n'
                            'with open(os.environ["CLARION_FAKE_GIO_LOG"],"a") as stream:\n'
                            ' stream.write(json.dumps({"argv":sys.argv[1:]},ensure_ascii=False)+"\\n")\n'
                            'sys.exit(17 if os.path.exists(os.environ["CLARION_FAKE_GIO_FAIL"]) else 0)\n')
        fake_gio.chmod(0o755)
        env.update(PATH=str(fake_bin) + os.pathsep + env.get("PATH", "/usr/bin:/bin"),
                   CLARION_FAKE_GIO_LOG=str(fake_log), CLARION_FAKE_GIO_FAIL=str(failure_flag))
        clarion = Path(os.environ.get("CLARION_APPS_CLARION_BIN", ROOT / "target/release/clarion")).resolve()
        launcher = Path(os.environ.get("CLARION_APPS_LAUNCHER_BIN", ROOT / "target/debug/clarion-launcher")).resolve()
        renderer = os.environ.get("CLARION_APPS_RENDERER", "gles")
        outer_renderer = os.environ.get("CLARION_APPS_OUTER_RENDERER", renderer)
        inner_renderer = os.environ.get("CLARION_APPS_INNER_RENDERER", renderer)
        require(all(value in ("gles", "vulkan") for value in (renderer, outer_renderer, inner_renderer)),
                "CLARION_APPS_RENDERER and outer/inner overrides must be gles or vulkan")
        result["binaries"] = {"compositor": str(clarion), "launcher": str(launcher)}
        result["renderer_requested"] = renderer
        result["rust_log"] = env["RUST_LOG"]
        result["renderer_requests"] = {"outer": outer_renderer, "inner": inner_renderer}
        inner_wayland_debug = os.environ.get("CLARION_APPS_INNER_WAYLAND_DEBUG") == "1"
        result["inner_compositor_wayland_client_trace"] = inner_wayland_debug
        result["renderer_evidence"] = {}
        def verify_renderer(label, path, expected):
            contents = path.read_text(errors="replace")
            native = "nested native Vulkan WSI renderer initialized" in contents
            fallback = "falling back to GLES" in contents
            evidence = {"log": str(path), "requested": expected, "native_vulkan_wsi_initialized": native,
                        "gles_fallback": fallback, "actual": "vulkan" if native else "gles"}
            result["renderer_evidence"][label] = evidence
            if expected == "vulkan":
                require(native and not fallback,
                        f"{label} did not initialize native Vulkan WSI; refusing GLES fallback: {evidence}")
            else:
                require("configured graphics renderer backend: Gles" in contents and not native,
                        f"{label} did not confirm requested GLES backend: {evidence}")
        result["requested_checks"] = {name: bool(getattr(args, name)) for name in
                                      ("toolbar", "fullscreen", "ime", "f4", "shell_native_test")}
        config = root / "nested.kdl"
        config.write_text('input { keyboard { xkb { layout "us"; }; }; }\n'
                          'output "winit" { scale 1; }\nxwayland-satellite { off; }\n'
                          'hotkey-overlay { skip-at-startup; }\n')
        run([str(clarion), "validate", "--config", str(config)], env)
        log = (artifacts / "compositor.log").open("w")
        logs.append(log)
        print("Starting isolated production Apps acceptance.", flush=True)
        compositor = subprocess.Popen([str(clarion), "--config", str(config), "--renderer", outer_renderer],
                                      env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        ipc_path = wait_for("test compositor IPC", lambda: next(runtime.glob("clarion.*.sock"), None), compositor)
        display = wait_for("test Wayland display", lambda: next(
            (p for p in runtime.glob("wayland-*") if p.is_socket()), None), compositor)
        require(display != parent_display, "Input would target the parent session")
        env.update(WAYLAND_DISPLAY=str(display), CLARION_SOCKET=str(ipc_path),
                   CLARION_SHELL_ISOLATED_TEST="1", CLARION_SHELL_PARENT_WAYLAND_DISPLAY=str(parent_display))
        def ipc(*command):
            return json.loads(run([str(clarion), "msg", "-j", *command], env))
        output = wait_for("test output", lambda: ipc("outputs").get("winit"), compositor)
        verify_renderer("outer", artifacts / "compositor.log", outer_renderer)
        require(not (runtime / "systemd/private").exists(), "Test unexpectedly has a user manager")
        daemon_log = (artifacts / "launcher.log").open("w")
        logs.append(daemon_log)
        daemon_command = [str(launcher), "--serve", "--output", "winit", "--parent-pid", str(os.getpid())]
        daemon = subprocess.Popen(daemon_command, env=env | {"WAYLAND_DEBUG": "client"}, stdout=daemon_log,
                                  stderr=subprocess.STDOUT, start_new_session=True)
        launcher_socket = runtime / "clarion-launcher.sock"
        wait_for("production launcher socket", launcher_socket.exists, daemon)
        peer = Peer(launcher_socket)
        require(not ipc("layers"), "Serve mode unexpectedly mapped a window")
        result.update(private_bus=True, isolated_desktops=True, fake_gio=str(fake_gio),
                      initial_output=output, launcher_pid=daemon.pid)
        def layers():
            return [layer for layer in ipc("layers") if layer["namespace"] == "clarion-launcher"]
        def assert_visible(process=None):
            found = wait_for("sole launcher Overlay", lambda: layers() or None, process or daemon)
            require(found == [{"namespace": "clarion-launcher", "output": "winit", "layer": "Overlay",
                               "keyboard_interactivity": "Exclusive"}], f"Unexpected launcher layers: {found}")
        def assert_hidden():
            wait_for("launcher layer hidden", lambda: not layers(), daemon)
            require(daemon.poll() is None, "Hiding terminated the daemon")
        def show(request, **fields):
            started = time.monotonic()
            peer.send("Show", request, output="winit", theme="dark", accent="#56a8ff", **fields)
            try:
                peer.expect("Visible", request, seconds=15 if request == 1 else 5)
            except helpers.SmokeFailure:
                if request == 1:
                    result["first_show_diagnostic"] = {"layers": ipc("layers"), "process": {}}
                    capture("first-show-timeout")
                    for name in ("status", "wchan", "stack"):
                        try:
                            result["first_show_diagnostic"]["process"][name] = Path(f"/proc/{daemon.pid}/{name}").read_text()
                        except OSError as error:
                            result["first_show_diagnostic"]["process"][name] = str(error)
                    peer.send("Show", 2001, output="winit", theme="dark", accent="#56a8ff")
                    try:
                        peer.expect("Visible", 2001)
                    except helpers.SmokeFailure as error:
                        result["first_show_diagnostic"]["retry"] = str(error)
                    result["first_show_diagnostic"]["retry_layers"] = ipc("layers")
                raise
            if request == 1:
                result["first_show_seconds"] = round(time.monotonic() - started, 3)
            assert_visible()
            time.sleep(0.15)
        def capture(name):
            path = artifacts / f"{name}.png"
            run(["grim", "-o", "winit", str(path)], env)
            require(path.stat().st_size > 1000, "Empty production UI screenshot")
            return path
        def key(*arguments):
            run(["wtype", *arguments], env)
        def search(text):
            key("-M", "ctrl", "-k", "a", "-m", "ctrl")
            key(text)
            key("-M", "ctrl", "-k", "a", "-k", "c", "-m", "ctrl")
            copied = run(["wl-paste", "--no-newline"], env)
            require(copied == text, f"Search field did not receive text: {copied!r}")
            time.sleep(0.1)
        def launch_selected(request, path, trigger=None):
            count = len(fake_log.read_text().splitlines())
            if trigger is None:
                key("-k", "Return")
            else:
                trigger()
            event = peer.expect("Launched", request)
            require(event["app_id"] == path.name, f"Wrong app selected: {event}")
            peer.expect("Hidden", request)
            assert_hidden()
            calls = [json.loads(line) for line in fake_log.read_text().splitlines()]
            require(len(calls) == count + 1 and calls[-1]["argv"] == ["launch", str(path)],
                    f"GIO argv was not literal or invoked twice: {calls}")
            return event
        print("Checking Suggested, localized search, literal launch and reopening.", flush=True)
        show(1, recent=[gamma.name])
        capture("apps-categories-suggested")
        result["checks"]["suggested"] = launch_selected(1, gamma)
        show(2)
        search("uniquealpha")
        capture("apps-search-ascii")
        result["checks"]["ascii_literal_launch"] = launch_selected(2, alpha)
        show(3)
        search("画板")
        capture("apps-search-chinese")
        result["checks"]["chinese_search"] = launch_selected(3, beta)
        show(4)
        key("-k", "Escape")
        peer.expect("Hidden", 4)
        assert_hidden()
        result["checks"]["escape_keeps_daemon_alive"] = True
        print("Checking rapid epochs, duplicate instance and launch failure.", flush=True)
        peer.send("Show", 100, output="winit")
        peer.send("Hide", 101)
        peer.send("Show", 102, output="winit")
        peer.expect("Hidden", 101)
        peer.expect("Visible", 102)
        assert_visible()
        peer.settle()
        require(not any(event["type"] == "Visible" and event.get("request_id") == 100 for event in peer.events),
                "Canceled Show reopened an obsolete epoch")
        peer.send("Hide", 103)
        peer.expect("Hidden", 103)
        assert_hidden()
        peer.settle(0.3)
        require(not layers(), "An obsolete Show reopened after Hide")
        result["checks"]["rapid_epochs"] = True
        inode = launcher_socket.stat().st_ino
        duplicate = subprocess.Popen(daemon_command, env=env, stdout=daemon_log,
                                     stderr=subprocess.STDOUT, start_new_session=True)
        require(duplicate.wait(timeout=3) == 0, "Duplicate daemon did not exit cleanly")
        duplicate = None
        require(launcher_socket.stat().st_ino == inode and daemon.poll() is None,
                "Duplicate daemon replaced the original socket/process")
        result["checks"]["duplicate_preserves_socket"] = True
        show(5)
        search("uniquealpha")
        failure_flag.touch()
        previous_calls = len(fake_log.read_text().splitlines())
        key("-k", "Return")
        error = peer.expect("Error", 5)
        require(error.get("context") == "launch" and error.get("message"), "Incomplete launch error event")
        assert_visible()
        require(len(fake_log.read_text().splitlines()) == previous_calls + 1, "Failed launch was retried")
        capture("apps-launch-failed")
        failure_flag.unlink()
        result["checks"]["launch_failure"] = error
        peer.send("Hide", 6)
        peer.expect("Hidden", 6)
        assert_hidden()
        print("Checking catalog Reload and preference commands.", flush=True)
        delta = desktop(applications, "org.clarion.Delta.desktop", "Delta Reader", "Delta 信息阅读",
                        "uniquedelta;阅读", "Viewer", icon_paths[3])
        peer.send("Reload", 7)
        time.sleep(0.4)
        peer.send("Preferences", 8, theme="light", accent="#cc6655", recent=[delta.name])
        peer.settle(0.1)
        show(9)
        search("uniquedelta")
        capture("apps-after-reload")
        result["checks"]["reload_discovers_fixture"] = launch_selected(9, delta)
        peer.send("Toggle", 10, output="winit")
        peer.expect("Visible", 10)
        assert_visible()
        peer.send("Toggle", 11)
        peer.expect("Hidden", 11)
        assert_hidden()
        result["checks"]["toggle"] = True
        show(12)
        search("unique")
        key("-k", "Down")
        capture("apps-keyboard-navigation")
        result["checks"]["keyboard_down_enter"] = launch_selected(12, beta)
        show(14)
        search("uniquealpha")
        capture("apps-before-tile-click")
        pointer = helpers.pointer_helper(root, env)
        logical = ipc("outputs")["winit"]["logical"]
        tile_x = round((logical["width"] - 980) / 2 + 92)
        tile_y = round((logical["height"] - 680) / 2 + 308)
        clicked = launch_selected(14, alpha, lambda: run(
            [str(pointer), str(tile_x), str(tile_y), str(logical["width"]), str(logical["height"])], env))
        result["checks"]["native_app_tile_click"] = {"event": clicked, "output_point": [tile_x, tile_y]}
        if args.toolbar:
            print("Checking real pointer toolbar controls and persisted view state.", flush=True)
            show(20)
            capture("apps-before-toolbar")
            pointer = helpers.pointer_helper(root, env)
            logical = ipc("outputs")["winit"]["logical"]
            left, top = (logical["width"] - 980) / 2, (logical["height"] - 680) / 2
            state_file = config_dir / "apps-view.json"
            def choose(x, mode, display, label):
                run([str(pointer), str(round(left + x)), str(round(top + args.toolbar_y)),
                     str(logical["width"]), str(logical["height"])], env)
                def saved():
                    try:
                        value = json.loads(state_file.read_text())
                        return value if value["mode"] == mode and value["display"] == display else None
                    except (OSError, ValueError):
                        return None
                state = wait_for(label, saved, daemon, seconds=3)
                # Persistence completes on a worker before the compositor has
                # necessarily presented the UI's next frame.
                time.sleep(0.25)
                capture(label)
                return state
            transitions = [choose(138, "name", "icons", "apps-name-icons"),
                           choose(281, "name", "list", "apps-name-list"),
                           choose(70, "category", "list", "apps-category-list"),
                           choose(225, "category", "icons", "apps-category-icons")]
            result["checks"]["toolbar_persisted"] = transitions
            peer.send("Hide", 21)
            peer.expect("Hidden", 21)
            assert_hidden()
        print("Checking independent IPC peers and owner disconnection.", flush=True)
        show(500)
        second = Peer(launcher_socket)
        second.send("Show", 500, output="winit")
        second.expect("Visible", 500)
        peer.expect("Hidden", 500)
        assert_visible()
        peer.send("Hide", 501)
        peer.expect("Hidden", 501)
        second.settle(0.2)
        require(not any(event["type"] == "Hidden" for event in second.events),
                "Inactive peer hid the active owner's window")
        assert_visible()
        result["checks"]["two_peer_ownership"] = {"same_request_id": 500, "second_events": second.events,
                                                   "inactive_hide_preserved_window": True}
        second.socket.close()
        assert_hidden()
        result["checks"]["owner_disconnect_hides"] = True
        if args.fullscreen:
            print("Checking Apps above a fullscreen native fixture and focus restoration.", flush=True)
            source = root / "fullscreen.c"
            source.write_text(r'''
#include <gtk/gtk.h>
#include <stdio.h>
static gboolean draw(GtkWidget *widget, cairo_t *cr, gpointer data) {
    cairo_set_source_rgb(cr, .08, .18, .25); cairo_paint(cr); return FALSE;
}
static gboolean key(GtkWidget *widget, GdkEventKey *event, gpointer data) {
    printf("fixture-key:%u\n", event->keyval); fflush(stdout); return FALSE;
}
int main(int argc, char **argv) {
    gtk_init(&argc, &argv);
    GtkWidget *window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_title(GTK_WINDOW(window), "Clarion isolated fullscreen fixture");
    gtk_window_set_decorated(GTK_WINDOW(window), FALSE);
    g_signal_connect(window, "destroy", G_CALLBACK(gtk_main_quit), NULL);
    g_signal_connect(window, "draw", G_CALLBACK(draw), NULL);
    g_signal_connect(window, "key-press-event", G_CALLBACK(key), NULL);
    gtk_window_fullscreen(GTK_WINDOW(window)); gtk_widget_show_all(window);
    gtk_main(); return 0;
}
''')
            binary = root / "fullscreen-fixture"
            import shlex
            flags = shlex.split(run(["pkg-config", "--cflags", "--libs", "gtk+-3.0"], env))
            run(["cc", str(source), "-o", str(binary), *flags], env)
            fullscreen_log_path = artifacts / "fullscreen.log"
            fullscreen_log = fullscreen_log_path.open("w")
            logs.append(fullscreen_log)
            fullscreen = subprocess.Popen([str(binary)], env=env | {
                "GDK_BACKEND": "wayland", "GSETTINGS_SCHEMA_DIR": "/usr/share/glib-2.0/schemas",
                "WAYLAND_DEBUG": "client"}, stdout=fullscreen_log, stderr=subprocess.STDOUT,
                start_new_session=True)
            logical = ipc("outputs")["winit"]["logical"]
            def fullscreen_window():
                return next((window for window in ipc("windows") if window.get("pid") == fullscreen.pid
                             and window["layout"]["window_size"] == [logical["width"], logical["height"]]), None)
            window = wait_for("fullscreen fixture bounds", fullscreen_window, fullscreen)
            capture("fullscreen-baseline")
            show(550)
            search("uniquealpha")
            capture("apps-above-fullscreen")
            key("-k", "Escape")
            peer.expect("Hidden", 550)
            assert_hidden()
            wait_for("fullscreen keyboard focus restored", lambda:
                     (ipc("focused-window") or {}).get("pid") == fullscreen.pid, fullscreen)
            key("f")
            wait_for("native key reaches previous fullscreen app", lambda:
                     "fixture-key:102" in fullscreen_log_path.read_text(errors="replace"), fullscreen)
            result["checks"]["fullscreen_and_focus"] = {"window": window, "overlay_visible": True,
                                                        "native_key_after_escape": "f"}
            stop(fullscreen)
            fullscreen = None
        require(preferences_path.read_bytes() == original_preferences, "Launcher rewrote shared Shell preferences")
        result["checks"]["shared_preferences_unchanged"] = True
        result["fake_gio_calls"] = [json.loads(line) for line in fake_log.read_text().splitlines()]
        print("Checking graceful shutdown and parent-death cleanup.", flush=True)
        peer.socket.close()
        result["events"] = peer.events
        peer = None
        stop(daemon)
        require(daemon.returncode == 0 and not launcher_socket.exists(), "Daemon shutdown did not remove its socket")
        result["checks"]["graceful_shutdown"] = {"exit_code": daemon.returncode, "socket_removed": True}
        pidfile = root / "guardian-child.pid"
        guardian_code = ('import os,sys,subprocess,time\n'
                         'from pathlib import Path\n'
                         'child=subprocess.Popen([sys.argv[1],"--serve","--output","winit","--parent-pid",str(os.getpid())],start_new_session=True)\n'
                         'Path(sys.argv[2]).write_text(str(child.pid))\n'
                         'while True: time.sleep(1)\n')
        guardian = subprocess.Popen([sys.executable, "-c", guardian_code, str(launcher), str(pidfile)],
                                    env=env, stdout=daemon_log, stderr=subprocess.STDOUT, start_new_session=True)
        wait_for("guardian child PID", pidfile.exists, guardian)
        orphan_pid = int(pidfile.read_text())
        wait_for("parent-owned daemon socket", launcher_socket.exists, guardian)
        owned_peer = Peer(launcher_socket)
        owned_peer.send("Show", 600, output="winit")
        owned_peer.expect("Visible", 600)
        assert_visible(guardian)
        guardian.kill()
        guardian.wait(timeout=3)
        def child_exited():
            try:
                state = Path(f"/proc/{orphan_pid}/stat").read_text().partition(")")[2].split()[0]
                return state == "Z"
            except FileNotFoundError:
                return True
        wait_for("launcher parent-death cleanup", lambda: not launcher_socket.exists() and not layers() and child_exited(), seconds=4)
        owned_peer.socket.close()
        result["checks"]["parent_death"] = {"parent_killed": True, "child_exited": True,
                                           "socket_removed": True, "layer_removed": True}
        orphan_pid = None
        if args.ime:
            print("Checking actual Pinyin composition in production Apps search.", flush=True)
            hello = desktop(applications, "org.clarion.Hello.desktop", "Hello Utility", "你好工具",
                            "你好;拼音", "Utility", icon_paths[0])
            keyboard = helpers.fixed_keyboard(root, env)
            inner_runtime = root / "ime-runtime"
            inner_runtime.mkdir(mode=0o700)
            inner_env = env | {"XDG_RUNTIME_DIR": str(inner_runtime)}
            inner_env.pop("CLARION_SOCKET", None)
            inner_log = (artifacts / "ime-compositor.log").open("w")
            logs.append(inner_log)
            inner_compositor = subprocess.Popen([str(clarion), "--config", str(config), "--renderer", inner_renderer],
                                               env=inner_env | ({"WAYLAND_DEBUG": "client"} if inner_wayland_debug else {}),
                                               stdout=inner_log, stderr=subprocess.STDOUT,
                                               start_new_session=True)
            inner_ipc_path = wait_for("inner IME compositor IPC", lambda:
                                     next(inner_runtime.glob("clarion.*.sock"), None), inner_compositor)
            inner_display = wait_for("inner IME display", lambda: next(
                (p for p in inner_runtime.glob("wayland-*") if p.is_socket()), None), inner_compositor)
            wait_for("outer focus on inner compositor", lambda:
                     (ipc("focused-window") or {}).get("pid") == inner_compositor.pid, inner_compositor)
            inner_env.update(WAYLAND_DISPLAY=str(inner_display), CLARION_SOCKET=str(inner_ipc_path))
            def inner_ipc(*command):
                return json.loads(run([str(clarion), "msg", "-j", *command], inner_env))
            wait_for("inner IME output", lambda: inner_ipc("outputs").get("winit"), inner_compositor)
            verify_renderer("ime_inner", artifacts / "ime-compositor.log", inner_renderer)
            fcitx_config = Path(env["XDG_CONFIG_HOME"]) / "fcitx5"
            fcitx_config.mkdir()
            (fcitx_config / "profile").write_text(
                '[Groups/0]\nName=LauncherTest\nDefault Layout=us\nDefaultIM=pinyin\n\n'
                '[Groups/0/Items/0]\nName=keyboard-us\n\n[Groups/0/Items/1]\nName=pinyin\n\n'
                '[GroupOrder]\n0=LauncherTest\n')
            (fcitx_config / "config").write_text('[Behavior]\nActiveByDefault=True\nShareInputState=All\n')
            ime_log = (artifacts / "fcitx5.log").open("w")
            logs.append(ime_log)
            ime = subprocess.Popen(["fcitx5", "-D", "--disable=xcb,xim,ibus,cloudpinyin"], env=inner_env,
                                   stdout=ime_log, stderr=subprocess.STDOUT, start_new_session=True)
            wait_for("private Pinyin service", lambda: "org.fcitx.Fcitx5" in run(
                ["busctl", "--address=" + env["DBUS_SESSION_BUS_ADDRESS"], "--no-pager", "list"], env), ime)
            ime_trace_path = artifacts / "production-ime.wayland.log"
            ime_trace = ime_trace_path.open("w")
            logs.append(ime_trace)
            ime_daemon = subprocess.Popen(daemon_command, env=inner_env | {"WAYLAND_DEBUG": "client"},
                                          stdout=ime_trace, stderr=subprocess.STDOUT, start_new_session=True)
            inner_socket = inner_runtime / "clarion-launcher.sock"
            wait_for("production IME daemon socket", inner_socket.exists, ime_daemon)
            ime_peer = Peer(inner_socket)
            ime_peer.send("Show", 700, output="winit", theme="dark", recent=[])
            ime_peer.expect("Visible", 700)
            wait_for("production IME layer", lambda: inner_ipc("layers"), ime_daemon)
            time.sleep(0.3)
            run(["fcitx5-remote", "-s", "pinyin"], inner_env)
            run(["fcitx5-remote", "-o"], inner_env)
            run([str(keyboard), "49", "23", "35", "30", "24"], env)
            def preedit_painted():
                trace = ime_trace_path.read_text(errors="replace")
                surface = re.search(r'get_layer_surface\([^\n]+wl_surface[@#](\d+),', trace)
                marks = list(re.finditer(r'zwp_text_input_v3[@#]\d+\.preedit_string\("([^"\n]+)"', trace))
                if not surface or not marks:
                    return None
                final_text = marks[-1].group(1)
                if re.sub(r"[^a-z]", "", final_text) != "nihao":
                    return None
                first_final = next(mark for mark in marks if mark.group(1) == final_text)
                return final_text if re.search(r'wl_surface[@#]' + surface.group(1) +
                    r'\.attach\([^\n]+\).*?wl_surface[@#]' + surface.group(1) +
                    r'\.commit\(', trace[first_final.end():], re.DOTALL) else None
            preedit_deadline = float(os.environ.get("CLARION_APPS_IME_FRAME_TIMEOUT", "0.5"))
            require(0.05 <= preedit_deadline <= 3, "IME frame timeout must be between 0.05 and 3 seconds")
            preedit_wait_started = time.monotonic()
            try:
                painted = wait_for("GPUI frame presents held IME preedit before another key",
                                   preedit_painted, ime_daemon, seconds=preedit_deadline)
            except helpers.SmokeFailure:
                run(["grim", "-o", "winit", str(artifacts / "apps-ime-preedit-timeout.png")], inner_env)
                result["checks"]["ime_preedit_painted"] = False
                raise
            result["checks"]["ime_preedit_painted"] = {"text": painted, "frame_before_commit": True,
                "wait_after_typing_seconds": round(time.monotonic() - preedit_wait_started, 3),
                "deadline_seconds": preedit_deadline}
            run(["grim", "-o", "winit", str(artifacts / "apps-ime-preedit.png")], inner_env)
            run([str(keyboard), "57"], env)
            time.sleep(0.15)
            run([str(keyboard), "+29", "30", "46", "-29"], env)
            committed = run(["wl-paste", "--no-newline"], inner_env)
            require(committed == "你好", f"Production IME query mismatch: {committed!r}")
            run(["grim", "-o", "winit", str(artifacts / "apps-ime-committed.png")], inner_env)
            before = len(fake_log.read_text().splitlines())
            run([str(keyboard), "28"], env)
            launched = ime_peer.expect("Launched", 700)
            require(launched["app_id"] == hello.name, f"IME query selected the wrong app: {launched}")
            ime_peer.expect("Hidden", 700)
            calls = [json.loads(line) for line in fake_log.read_text().splitlines()]
            require(len(calls) == before + 1 and calls[-1]["argv"] == ["launch", str(hello)],
                    "IME selection did not invoke the exact fake GIO fixture")
            ime_trace.flush()
            trace = ime_trace_path.read_text(errors="replace")
            require(re.search(r'zwp_text_input_v3[@#]\d+\.preedit_string\("[^"\n]+"', trace)
                    and re.search(r'zwp_text_input_v3[@#]\d+\.commit_string\("你好"\)', trace),
                    "Missing real production IME preedit/commit protocol evidence")
            result["checks"]["production_ime"] = {"query": committed, "launched": launched,
                                                    "native_keyboard_route": "outer -> inner winit -> Fcitx grab",
                                                    "preedit_and_commit": True}
            ime_peer.socket.close()
            stop(ime_daemon)
            ime_daemon = None
            stop(ime)
            ime = None
            stop(inner_compositor)
            inner_compositor = None
        if args.f4:
            print("Checking native F4 through a full private release Shell.", flush=True)
            shell_binary = Path(os.environ.get("CLARION_APPS_SHELL_BIN", ROOT /
                "clarion-shell/build/linux/x64/release/bundle/clarion-shell")).resolve()
            require(shell_binary.is_file(), f"Release Shell binary is absent: {shell_binary}")
            keyboard = helpers.fixed_keyboard(root, env)
            f4_runtime = root / "f4-runtime"
            f4_runtime.mkdir(mode=0o700)
            f4_cache = root / "f4-cache"
            f4_cache.mkdir(mode=0o700)
            f4_env = env | {"XDG_RUNTIME_DIR": str(f4_runtime),
                           "XDG_CACHE_HOME": str(f4_cache),
                           "PATH": str(fake_bin) + os.pathsep + str(ROOT / "resources") + os.pathsep + env["PATH"],
                           "DBUS_SYSTEM_BUS_ADDRESS": env["DBUS_SESSION_BUS_ADDRESS"],
                           "CLARION_LAUNCHER_BIN": str(launcher), "GDK_BACKEND": "wayland",
                           "GSETTINGS_SCHEMA_DIR": "/usr/share/glib-2.0/schemas"}
            for variable in ("CLARION_SOCKET", "FLUTTER_TEST"):
                f4_env.pop(variable, None)
            preferences_path.write_text(json.dumps(preferences | {
                "shell_style": "macos", "use_external_launcher": True}) + "\n")
            f4_log = (artifacts / "f4-compositor.log").open("w")
            logs.append(f4_log)
            inner_compositor = subprocess.Popen([str(clarion), "--config", str(config), "--renderer", inner_renderer],
                env=f4_env | ({"WAYLAND_DEBUG": "client"} if inner_wayland_debug else {}),
                stdout=f4_log, stderr=subprocess.STDOUT, start_new_session=True)
            f4_ipc_path = wait_for("native F4 compositor IPC", lambda:
                next(f4_runtime.glob("clarion.*.sock"), None), inner_compositor)
            f4_display = wait_for("native F4 display", lambda: next(
                (path for path in f4_runtime.glob("wayland-*") if path.is_socket()), None), inner_compositor)
            wait_for("outer focus on F4 compositor", lambda:
                (ipc("focused-window") or {}).get("pid") == inner_compositor.pid, inner_compositor)
            f4_env.update(WAYLAND_DISPLAY=str(f4_display), CLARION_SOCKET=str(f4_ipc_path))
            def f4_ipc(*command):
                return json.loads(run([str(clarion), "msg", "-j", *command], f4_env))
            wait_for("native F4 output", lambda: f4_ipc("outputs").get("winit"), inner_compositor)
            verify_renderer("f4_inner", artifacts / "f4-compositor.log", inner_renderer)
            shell_log = (artifacts / "f4-shell.log").open("w")
            logs.append(shell_log)
            shell_process = subprocess.Popen([str(shell_binary), "--monitor=0", "--no-wallpaper",
                "--no-tray", "--no-notifications"], env=f4_env, stdout=shell_log,
                stderr=subprocess.STDOUT, start_new_session=True)
            shell_socket = f4_runtime / "clarion-shell.sock"
            wait_for("full private Shell IPC", shell_socket.exists, shell_process, seconds=20)
            def collapsed_shell():
                current = f4_ipc("layers")
                shell = next((layer for layer in current if layer["namespace"] == "clarion-shell"), None)
                return current if shell and shell["layer"] == "Top" and not any(
                    layer["namespace"] == "clarion-launcher" for layer in current) else None
            initial_layers = wait_for("full Shell mapped and collapsed", collapsed_shell, shell_process)
            f4_started = time.monotonic()
            run([str(keyboard), "62"], env)
            def opened_by_f4():
                current = f4_ipc("layers")
                by_name = {layer["namespace"]: layer for layer in current}
                return current if (by_name.get("clarion-launcher", {}).get("keyboard_interactivity") == "Exclusive"
                    and by_name.get("clarion-shell", {}).get("layer") == "Overlay"
                    and by_name.get("clarion-shell", {}).get("keyboard_interactivity") == "None") else None
            opened_layers = wait_for("native F4 launches GPUI through actual Shell IPC", opened_by_f4,
                                     shell_process, seconds=15)
            first_f4_seconds = round(time.monotonic() - f4_started, 3)
            run(["grim", "-o", "winit", str(artifacts / "apps-native-f4-opened.png")], f4_env)
            run([str(keyboard), "62"], env)
            closed_layers = wait_for("second native F4 collapses actual Shell", collapsed_shell, shell_process)
            run(["grim", "-o", "winit", str(artifacts / "apps-native-f4-closed.png")], f4_env)
            result["checks"]["native_f4"] = {"shell_binary": str(shell_binary),
                "route": "outer fixed US KEY_F4 -> inner native winit -> built-in bind -> actual Shell IPC -> GPUI",
                "separate_initially_empty_cache": True, "first_f4_open_seconds": first_f4_seconds,
                "shell_socket_private": True, "initial_layers": initial_layers,
                "opened_layers": opened_layers, "closed_layers": closed_layers}
            stop(shell_process)
            shell_process = None
            wait_for("full Shell launcher child cleanup", lambda:
                     not (f4_runtime / "clarion-launcher.sock").exists() and not any(
                         layer["namespace"] == "clarion-launcher" for layer in f4_ipc("layers")), seconds=4)
            stop(inner_compositor)
            inner_compositor = None
        if args.shell_native_test:
            print("Checking production Flutter LauncherBridge and real Dock route.", flush=True)
            test_root = root / "shell-test"
            test_root.mkdir()
            captures = artifacts / "shell-native"
            captures.mkdir()
            shell_env = env | {
                "CLARION_SHELL_TEST_ROOT": str(test_root), "CLARION_SHELL_CAPTURE_DIR": str(captures),
                "CLARION_SHELL_PARENT_SOCKET": os.environ.get("CLARION_SOCKET", "parent-session"),
                "CLARION_SHELL_CLARION_BIN": str(clarion), "CLARION_LAUNCHER_BIN": str(launcher),
                "CLARION_SHELL_POINTER_HELPER": str(helpers.pointer_helper(root, env)),
                "CLARION_SHELL_WTYPE": shutil.which("wtype"), "CLARION_SHELL_GRIM": shutil.which("grim"),
                "CLARION_SHELL_SWAYBG": shutil.which("swaybg"), "GDK_BACKEND": "wayland",
                "CLARION_LAUNCHER_TEST_OUTPUT": "winit",
            }
            shell_log = (artifacts / "shell-native.log").open("w")
            logs.append(shell_log)
            flutter = os.environ.get("CLARION_FLUTTER_BIN") or str(Path.home() / ".cache/realand/toolchains/flutter/bin/flutter")
            shell_process = subprocess.Popen(
                [flutter, "test", "--no-pub", "-d", "linux", "integration_test/gpui_launcher_native_test.dart"],
                cwd=ROOT / "clarion-shell", env=shell_env, stdout=shell_log,
                stderr=subprocess.STDOUT, start_new_session=True)
            require(shell_process.wait(timeout=240) == 0, "Production Flutter native test failed; inspect shell-native.log")
            shell_process = None
            evidence = captures / "gpui-launcher-native-evidence.json"
            require(evidence.is_file(), "Production Shell test did not write evidence")
            result["checks"]["shell_native"] = json.loads(evidence.read_text())
            require(result["checks"]["shell_native"].get("passed") is True, "Production Shell evidence did not pass")
            wait_for("production bridge child/socket cleanup", lambda: not launcher_socket.exists() and not layers(), seconds=3)
        result["fake_gio_calls"] = [json.loads(line) for line in fake_log.read_text().splitlines()]
        result["passed"] = True
    except (helpers.SmokeFailure, OSError, subprocess.SubprocessError, ValueError) as error:
        result["error"] = str(error)
        if "ipc" in locals() and env.get("WAYLAND_DISPLAY", "").startswith(str(root)):
            try:
                result["failure_layers"] = ipc("layers")
                capture("failure-private-output")
            except (helpers.SmokeFailure, OSError, subprocess.SubprocessError):
                pass
        print(f"Production Apps acceptance failed: {error}", file=sys.stderr, flush=True)
    finally:
        if peer:
            result["events"] = peer.events
            peer.socket.close()
        stop(duplicate)
        stop(ime_daemon)
        stop(ime)
        stop(inner_compositor)
        stop(fullscreen)
        stop(shell_process)
        stop(guardian)
        if orphan_pid is not None:
            try:
                command = Path(f"/proc/{orphan_pid}/cmdline").read_bytes()
                if str(launcher).encode() in command:
                    os.killpg(orphan_pid, signal.SIGKILL)
            except (FileNotFoundError, ProcessLookupError):
                pass
        stop(daemon)
        if daemon is not None:
            result["daemon_exit_code"] = daemon.returncode
            result["socket_removed_on_exit"] = launcher_socket is not None and not launcher_socket.exists()
            if result["passed"] and (daemon.returncode != 0 or not result["socket_removed_on_exit"]):
                result["passed"] = False
                result["error"] = "Launcher did not shut down gracefully and remove its socket"
        stop(compositor)
        result["cleanup"] = True
        private.cleanup()
        for log in logs:
            log.close()
        write_json(artifacts / "result.json", result)
    print(f"Evidence: {artifacts / 'result.json'}", flush=True)
    return 0 if result["passed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--toolbar", action="store_true", help="Also click toolbar controls and inspect apps-view.json")
    parser.add_argument("--shell-native-test", action="store_true", help="Run the production Flutter LauncherBridge native test")
    parser.add_argument("--ime", action="store_true", help="Exercise actual private Pinyin composition and filtered app launch")
    parser.add_argument("--fullscreen", action="store_true", help="Check layering and focus restoration over a native fullscreen fixture")
    parser.add_argument("--f4", action="store_true", help="Check real F4 input with a full private release Shell and native keyboard route")
    parser.add_argument("--toolbar-y", type=int, default=185, help="Toolbar center relative to the 980x680 surface")
    parser.add_argument("--private-session", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.private_session:
        return exercise(args)
    display = Path(os.environ["WAYLAND_DISPLAY"])
    if not display.is_absolute():
        display = Path(os.environ["XDG_RUNTIME_DIR"]) / display
    require(display.is_socket(), "A parent Wayland display is required")
    with tempfile.TemporaryDirectory(prefix="clarion-apps-private-bus-") as temporary:
        env = os.environ | {"CLARION_APPS_PARENT_WAYLAND": str(display.resolve()),
                            "CLARION_APPS_PARENT_DBUS": os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")}
        for name in ("DBUS_SESSION_BUS_ADDRESS", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DISPLAY", "NOTIFY_SOCKET"):
            env.pop(name, None)
        for name, directory in (("XDG_RUNTIME_DIR", "runtime"), ("XDG_CONFIG_HOME", "config"),
                                ("XDG_DATA_HOME", "data"), ("XDG_CACHE_HOME", "cache")):
            path = Path(temporary) / directory
            path.mkdir(mode=0o700)
            env[name] = str(path)
        config = Path(temporary) / "bus.conf"
        config.write_text('<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen>'
                          '<auth>EXTERNAL</auth><policy context="default">'
                          '<allow send_destination="*" eavesdrop="true"/>'
                          '<allow eavesdrop="true"/><allow own="*"/></policy></busconfig>')
        command = ["dbus-run-session", "--config-file=" + str(config), "--", sys.executable,
                   str(Path(__file__).resolve()), "--artifacts", str(args.artifacts.resolve()),
                   "--toolbar-y", str(args.toolbar_y), "--private-session"]
        if args.toolbar:
            command.append("--toolbar")
        if args.shell_native_test:
            command.append("--shell-native-test")
        if args.ime:
            command.append("--ime")
        if args.fullscreen:
            command.append("--fullscreen")
        if args.f4:
            command.append("--f4")
        bus = subprocess.Popen(command, env=env, start_new_session=True)
        try:
            return bus.wait()
        finally:
            stop(bus)


if __name__ == "__main__":
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    raise SystemExit(main())
