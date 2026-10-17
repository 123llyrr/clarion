#!/usr/bin/env python3
"""Exercise the GPUI spike in a disposable, private-bus nested Clarion.

Only the nested compositor is connected to the parent Wayland socket. All input,
clipboard, screenshots and IPC use the newly discovered private test sockets.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import shlex
import struct
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]
NAMESPACE = "clarion-launcher"


class SmokeFailure(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise SmokeFailure(message)


def wait_for(description, predicate, process=None, seconds=12):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if process is not None and process.poll() is not None:
            raise SmokeFailure(f"{description}: process exited with {process.returncode}")
        result = predicate()
        if result:
            return result
        time.sleep(0.08)
    raise SmokeFailure(f"Timed out waiting for {description}")


def run(command, env, timeout=8):
    result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=timeout)
    require(result.returncode == 0,
            f"Command failed ({result.returncode}): {command!r}\n{result.stderr[-2000:]}")
    return result.stdout


def stop(process):
    if process is None:
        return
    # Every owned persistent process gets its own process group at creation.
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=5)


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def check_trace(path, output_logical=None):
    trace = path.read_text(errors="replace")
    role = re.findall(r"zwlr_layer_shell_v1[@#]\d+\.get_layer_surface\([^\n]+", trace)
    require(len(role) == 1, f"Expected exactly one layer role, got {len(role)}")
    require('"clarion-launcher"' in role[0], "Unexpected layer namespace")
    require(re.search(r', 3, "clarion-launcher"\)', role[0]), "Layer role is not Overlay")
    require(".get_xdg_surface(" not in trace and ".get_toplevel(" not in trace,
            "Layer spike acquired an xdg-shell role")
    surface_match = re.search(r"wl_surface[@#](\d+)", role[0])
    require(surface_match, "Cannot identify the layer wl_surface")
    surface = surface_match.group(1)
    layer_match = re.search(r"zwlr_layer_surface_v1[@#](\d+)", role[0])
    require(layer_match, "Cannot identify the layer-surface object")
    layer = layer_match.group(1)
    prefix = rf"zwlr_layer_surface_v1[@#]{layer}"
    for request in (r"set_size\(980, 680\)", r"set_anchor\(0\)",
                    r"set_exclusive_zone\(-1\)", r"set_keyboard_interactivity\(1\)"):
        require(re.search(prefix + r"\." + request, trace), f"Missing layer request {request}")
    configure = re.search(prefix + r"\.configure\((\d+), (\d+), (\d+)\)", trace)
    require(configure, "No layer configure received")
    serial, width, height = map(int, configure.groups())
    expected = (980, 680) if output_logical is None else (
        min(980, output_logical["width"]), min(680, output_logical["height"]))
    require((width, height) == expected, f"Unexpected logical configure: {width}x{height}; expected {expected}")
    ack = re.search(prefix + rf"\.ack_configure\({serial}\)", trace)
    attach = re.search(rf"wl_surface[@#]{surface}\.attach\(wl_buffer[@#]\d+", trace)
    require(ack and attach and configure.start() < ack.start() < attach.start(),
            "First buffer must be attached after the first configure is acknowledged")
    require(re.search(r"wl_keyboard[@#]\d+\.enter\([^\n]*wl_surface[@#]" + surface + r"\b", trace),
            "No keyboard enter for the layer surface")
    return {
        "layer_roles": len(role), "xdg_roles": 0,
        "logical_configure": [width, height],
        "configure_ack_before_first_buffer": True, "keyboard_enter": True,
        "text_input_enabled": bool(re.search(r"zwp_text_input_v3[@#]\d+\.enable\(", trace)),
    }


def png_size(path):
    header = path.read_bytes()[:24]
    require(header[:8] == b"\x89PNG\r\n\x1a\n" and len(header) == 24, "Invalid screenshot")
    return list(struct.unpack(">II", header[16:24]))


def fixed_keyboard(private, env):
    """Build a test keyboard whose evdev codes survive the nested winit hop."""
    protocol = next((Path.home() / ".cargo/registry/src").glob(
        "*/wayland-protocols-misc-*/protocols/virtual-keyboard-unstable-v1.xml"), None)
    require(protocol, "Cached virtual-keyboard protocol XML is required for the IME test")
    for executable in ("cc", "wayland-scanner", "pkg-config"):
        require(shutil.which(executable), f"Missing fixed-keyboard build tool: {executable}")
    header, generated = private / "virtual-keyboard.h", private / "virtual-keyboard.c"
    run(["wayland-scanner", "client-header", str(protocol), str(header)], env)
    run(["wayland-scanner", "private-code", str(protocol), str(generated)], env)
    source = private / "fixed-keyboard.c"
    source.write_text(r'''
#define _GNU_SOURCE
#include <wayland-client.h>
#include <xkbcommon/xkbcommon.h>
#include <sys/mman.h>
#include <unistd.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "virtual-keyboard.h"
static struct wl_seat *seat;
static struct zwp_virtual_keyboard_manager_v1 *manager;
static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version) {
    if (!strcmp(interface, "wl_seat"))
        seat = wl_registry_bind(registry, name, &wl_seat_interface, version < 7 ? version : 7);
    if (!strcmp(interface, "zwp_virtual_keyboard_manager_v1"))
        manager = wl_registry_bind(registry, name, &zwp_virtual_keyboard_manager_v1_interface, 1);
}
static void removed(void *data, struct wl_registry *registry, uint32_t name) {}
static const struct wl_registry_listener listener = {global, removed};
static void key(struct zwp_virtual_keyboard_v1 *keyboard, struct xkb_state *state,
                unsigned code, unsigned pressed) {
    zwp_virtual_keyboard_v1_key(keyboard, 0, code, pressed);
    xkb_state_update_key(state, code + 8, pressed ? XKB_KEY_DOWN : XKB_KEY_UP);
    zwp_virtual_keyboard_v1_modifiers(keyboard,
        xkb_state_serialize_mods(state, XKB_STATE_MODS_DEPRESSED),
        xkb_state_serialize_mods(state, XKB_STATE_MODS_LATCHED),
        xkb_state_serialize_mods(state, XKB_STATE_MODS_LOCKED), 0);
}
int main(int argc, char **argv) {
    struct wl_display *display = wl_display_connect(NULL);
    if (!display) return 2;
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &listener, NULL);
    if (wl_display_roundtrip(display) < 0 || !seat || !manager) return 3;
    struct xkb_context *context = xkb_context_new(0);
    struct xkb_rule_names names = {.layout = "us"};
    struct xkb_keymap *keymap = xkb_keymap_new_from_names(context, &names, 0);
    if (!keymap) return 4;
    char *text = xkb_keymap_get_as_string(keymap, XKB_KEYMAP_FORMAT_TEXT_V1);
    size_t length = strlen(text) + 1;
    int fd = memfd_create("clarion-test-keymap", MFD_CLOEXEC);
    if (fd < 0 || write(fd, text, length) != length) return 5;
    struct zwp_virtual_keyboard_v1 *keyboard =
        zwp_virtual_keyboard_manager_v1_create_virtual_keyboard(manager, seat);
    zwp_virtual_keyboard_v1_keymap(keyboard, 1, fd, length);
    struct xkb_state *state = xkb_state_new(keymap);
    for (int index = 1; index < argc; index++) {
        char *argument = argv[index];
        unsigned code = strtoul(argument[0] == '+' || argument[0] == '-' ? argument + 1 : argument, NULL, 10);
        if (code > 255) return 6;
        key(keyboard, state, code, argument[0] != '-');
        wl_display_roundtrip(display);
        usleep(70000);
        if (argument[0] != '+' && argument[0] != '-') {
            key(keyboard, state, code, 0);
            wl_display_roundtrip(display);
            usleep(20000);
        }
    }
    zwp_virtual_keyboard_v1_destroy(keyboard);
    wl_display_roundtrip(display);
    wl_display_disconnect(display);
    return 0;
}
''')
    binary = private / "fixed-keyboard"
    flags = shlex.split(run(["pkg-config", "--cflags", "--libs", "wayland-client", "xkbcommon"], env))
    run(["cc", str(source), str(generated), "-I", str(private), "-o", str(binary), *flags], env)
    return binary


def pointer_helper(private, env):
    protocol = next((Path.home() / ".cargo/registry/src").glob(
        "*/wayland-protocols-wlr-*/wlr-protocols/unstable/wlr-virtual-pointer-unstable-v1.xml"), None)
    require(protocol, "Cached virtual-pointer XML is required for the Shell test")
    source = ROOT / "scripts/fixtures/launcher_spike_pointer.c"
    require(source.is_file(), "Missing isolated native pointer fixture")
    header = private / "wlr-virtual-pointer-unstable-v1-client-protocol.h"
    generated = private / "wlr-virtual-pointer-protocol.c"
    run(["wayland-scanner", "client-header", str(protocol), str(header)], env)
    run(["wayland-scanner", "private-code", str(protocol), str(generated)], env)
    binary = private / "launcher-spike-pointer"
    flags = shlex.split(run(["pkg-config", "--cflags", "--libs", "wayland-client"], env))
    run(["cc", str(source), str(generated), "-I", str(private), "-o", str(binary), *flags], env)
    return binary


def exercise(args):
    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    report = {"passed": False, "scope": "isolated GPUI native smoke",
              "exclusions": ["Flutter backdrop", "Dock interaction", "physical multi-output", "previous application focus restoration"],
              "cycles": [], "cleanup": False}
    compositor = launcher = ime = inner_compositor = shell_process = None
    logs = []
    try:
        for executable in ("wtype", "wl-paste", "grim"):
            require(shutil.which(executable), f"Missing executable: {executable}")
        clarion = ROOT / "target/release/clarion"
        launcher_binary = ROOT / "target/debug/clarion-launcher"
        for binary in (clarion, launcher_binary):
            require(binary.is_file() and os.access(binary, os.X_OK), f"Build first: {binary}")
        require(os.environ.get("DBUS_SESSION_BUS_ADDRESS") !=
                os.environ.get("CLARION_TEST_PARENT_DBUS_ADDRESS"), "D-Bus session was not isolated")
        parent_display = Path(os.environ["CLARION_TEST_PARENT_WAYLAND"])
        require(parent_display.is_absolute() and parent_display.is_socket(), "Invalid parent Wayland socket")
        with tempfile.TemporaryDirectory(prefix="clarion-launcher-smoke-") as temporary:
            private = Path(temporary)
            env = os.environ.copy()
            for key in ("DISPLAY", "WAYLAND_SOCKET", "WAYLAND_DEBUG", "CLARION_SOCKET", "REALAND_SOCKET",
                        "CLARION_CONFIG", "REALAND_CONFIG", "NOTIFY_SOCKET", "WATCHDOG_PID",
                        "WATCHDOG_USEC", "CLARION_SHELL_SOCKET", "REALAND_SHELL_SOCKET"):
                env.pop(key, None)
            for variable, directory in (("XDG_RUNTIME_DIR", "runtime"), ("XDG_CONFIG_HOME", "config"),
                                        ("XDG_DATA_HOME", "data"), ("XDG_CACHE_HOME", "cache"),
                                        ("XDG_STATE_HOME", "state")):
                target = private / directory
                target.mkdir(mode=0o700)
                env[variable] = str(target)
            env.update(WAYLAND_DISPLAY=str(parent_display), WINIT_UNIX_BACKEND="wayland",
                       CLARION_DISABLE_SYSTEM_MANAGER_NOTIFY="1", XDG_SESSION_TYPE="wayland",
                       XDG_CURRENT_DESKTOP="clarion", RUST_LOG="clarion=info")
            runtime = Path(env["XDG_RUNTIME_DIR"])
            config = private / "nested.kdl"
            config.write_text('input {\n keyboard {\n  xkb { layout "us"; }\n }\n}\n'
                              'output "winit" { scale 1; }\n'
                              'xwayland-satellite { off; }\n'
                              'hotkey-overlay { skip-at-startup; }\n')
            shutil.copyfile(config, artifacts / "nested-config.kdl")
            run([str(clarion), "validate", "--config", str(config)], env)
            compositor_log = (artifacts / "compositor.log").open("w")
            logs.append(compositor_log)
            print("Starting disposable nested Clarion on a private D-Bus session.", flush=True)
            compositor = subprocess.Popen([str(clarion), "--config", str(config), "--renderer", "gles"],
                                          env=env, stdout=compositor_log, stderr=subprocess.STDOUT,
                                          start_new_session=True)
            ipc = wait_for("nested IPC socket", lambda: next(runtime.glob("clarion.*.sock"), None), compositor)
            display = wait_for("nested Wayland socket", lambda: next(
                (p for p in runtime.glob("wayland-*") if p.is_socket()), None), compositor)
            require(display != parent_display, "Test input would target the parent compositor")
            env.update(WAYLAND_DISPLAY=str(display), CLARION_SOCKET=str(ipc),
                       CLARION_LAUNCHER_ISOLATED_TEST="1")
            # Update this private bus only, so auto-activated portal/IME clients
            # inherit the nested socket and private runtime rather than the host.
            if shutil.which("dbus-update-activation-environment"):
                run(["dbus-update-activation-environment", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR",
                     "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME",
                     "XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE"], env)
            def ipc_json(*command):
                return json.loads(run([str(clarion), "msg", "-j", *command], env))
            outputs = wait_for("nested output", lambda: ipc_json("outputs"), compositor)
            outer_env = env.copy()
            require("winit" in outputs, f"Unexpected nested outputs: {list(outputs)}")
            require(not ipc_json("layers"), "Fresh nested session has unexpected layer clients")
            report.update(parent_wayland=str(parent_display), nested_wayland=str(display),
                          private_bus=True, isolated_config_and_runtime=True, initial_outputs=outputs)
            scales = [1, 2] if args.scale2 else [1]
            for scale in scales:
                if scale != 1:
                    run([str(clarion), "msg", "output", "winit", "scale", str(scale)], env)
                    wait_for("output scale", lambda: ipc_json("outputs")["winit"]["logical"]["scale"] == scale,
                             compositor)
                for cycle_number in range(1, 3):
                    tag = f"scale-{scale}-cycle-{cycle_number}"
                    print(f"Testing {tag}: layer role, text, screenshot and Escape.", flush=True)
                    trace_path = artifacts / f"{tag}.wayland.log"
                    trace_file = trace_path.open("w")
                    logs.append(trace_file)
                    client_env = env | {"WAYLAND_DEBUG": "client"}
                    launcher = subprocess.Popen([str(launcher_binary), "--spike", "--output", "winit",
                                                 "--quit-after", "10"], env=client_env,
                                                stdout=trace_file, stderr=subprocess.STDOUT,
                                                start_new_session=True)
                    expected = {"namespace": NAMESPACE, "output": "winit", "layer": "Overlay",
                                "keyboard_interactivity": "Exclusive"}
                    def find_layer():
                        trace_file.flush()
                        error_text = trace_path.read_text(errors="replace")
                        require("clarion-launcher: " not in error_text,
                                "Launcher reported an error: " + next(
                                    (line for line in error_text.splitlines() if "clarion-launcher: " in line), ""))
                        return next((layer for layer in ipc_json("layers") if layer == expected), None)
                    layer = wait_for("mapped Exclusive Overlay", find_layer, launcher)
                    time.sleep(0.3)
                    sample = f"Clarion input {scale}-{cycle_number}"
                    run(["wtype", "-d", "8", sample], env)
                    run(["wtype", "-M", "ctrl", "-k", "a", "-k", "c", "-m", "ctrl"], env)
                    copied = wait_for("typed text clipboard", lambda: (
                        value if (value := run(["wl-paste", "--no-newline"], env, timeout=2)) == sample
                        else None), launcher)
                    screenshot = artifacts / f"{tag}.png"
                    run(["grim", "-o", "winit", str(screenshot)], env)
                    require(screenshot.stat().st_size > 1000, "Screenshot is unexpectedly empty")
                    run(["wtype", "-k", "Escape"], env)
                    try:
                        code = launcher.wait(timeout=3)
                    except subprocess.TimeoutExpired as error:
                        raise SmokeFailure("Escape did not close the spike before its 10-second fallback") from error
                    require(code == 0, f"Spike exited with {code}")
                    launcher = None
                    wait_for("layer removal", lambda: not any(
                        layer["namespace"] == NAMESPACE for layer in ipc_json("layers")), compositor)
                    trace_file.flush()
                    logical = ipc_json("outputs")["winit"]["logical"]
                    cycle = {"name": tag, "layer": layer, "text_round_trip": copied,
                             "escape_exit_code": code, "layer_removed": True,
                             "output_logical": logical, "screenshot_pixels": png_size(screenshot),
                             "requested_panel_fits_output": logical["width"] >= 980 and logical["height"] >= 680,
                             "trace": check_trace(trace_path, logical)}
                    report["cycles"].append(cycle)
                    write_json(artifacts / "result.json", report)
            for name, output_name, lifetime in (("missing-output", "__clarion_spike_missing__", "3"),
                                                 ("timer-exit", "winit", "1")):
                print(f"Testing {name}: bounded exit and no residual layer.", flush=True)
                trace_path = artifacts / f"{name}.wayland.log"
                trace_file = trace_path.open("w")
                logs.append(trace_file)
                started = time.monotonic()
                launcher = subprocess.Popen([str(launcher_binary), "--spike", "--output", output_name,
                                             "--quit-after", lifetime], env=env | {"WAYLAND_DEBUG": "client"},
                                            stdout=trace_file, stderr=subprocess.STDOUT,
                                            start_new_session=True)
                code = launcher.wait(timeout=5)
                launcher = None
                elapsed = time.monotonic() - started
                trace_file.flush()
                require(not any(layer["namespace"] == NAMESPACE for layer in ipc_json("layers")),
                        f"{name} left a layer surface")
                result = {"exit_code": code, "elapsed_seconds": round(elapsed, 3), "layer_removed": True}
                if name == "missing-output":
                    trace = trace_path.read_text(errors="replace")
                    require(code != 0 and "was not found" in trace, "Invalid output did not report a failure")
                    require(".get_layer_surface(" not in trace, "Invalid output created a layer role")
                    result["diagnostic"] = "requested output was not found"
                else:
                    require(code == 0, "Timer-only spike did not exit successfully")
                    result["trace"] = check_trace(trace_path, ipc_json("outputs")["winit"]["logical"])
                report[name] = result
            if args.ime:
                print("Testing Chinese preedit/candidates/commit with private fcitx5 Pinyin.", flush=True)
                if args.scale2:
                    run([str(clarion), "msg", "output", "winit", "scale", "1"], env)
                    wait_for("outer scale restored for native IME route", lambda:
                             ipc_json("outputs")["winit"]["logical"]["scale"] == 1, compositor)
                    time.sleep(0.2)
                # Smithay's virtual keyboard sends directly to wl_keyboard and
                # bypasses input-method grabs. A second nested compositor turns
                # test-only outer keyboard events into native inner winit input.
                keyboard_env = env.copy()
                keyboard = fixed_keyboard(private, keyboard_env)
                inner_runtime = private / "ime-runtime"
                inner_runtime.mkdir(mode=0o700)
                inner_env = env | {"XDG_RUNTIME_DIR": str(inner_runtime)}
                inner_env.pop("CLARION_SOCKET", None)
                inner_log = (artifacts / "ime-compositor.log").open("w")
                logs.append(inner_log)
                inner_compositor = subprocess.Popen(
                    [str(clarion), "--config", str(config), "--renderer", "gles"], env=inner_env,
                    stdout=inner_log, stderr=subprocess.STDOUT, start_new_session=True)
                inner_ipc = wait_for("inner IME IPC", lambda: next(inner_runtime.glob("clarion.*.sock"), None),
                                     inner_compositor)
                inner_display = wait_for("inner IME display", lambda: next(
                    (p for p in inner_runtime.glob("wayland-*") if p.is_socket()), None), inner_compositor)
                wait_for("outer focus on inner compositor", lambda: (
                    ipc_json("focused-window") or {}).get("pid") == inner_compositor.pid, inner_compositor)
                env = inner_env | {"WAYLAND_DISPLAY": str(inner_display), "CLARION_SOCKET": str(inner_ipc)}
                wait_for("inner IME output", lambda: "winit" in ipc_json("outputs"), inner_compositor)
                for executable in ("fcitx5", "fcitx5-remote", "busctl"):
                    require(shutil.which(executable), f"Missing IME executable: {executable}")
                require(Path("/usr/share/fcitx5/inputmethod/pinyin.conf").exists(), "Pinyin is not installed")
                fcitx_config = Path(env["XDG_CONFIG_HOME"]) / "fcitx5"
                fcitx_config.mkdir()
                profile = ('[Groups/0]\nName=LauncherTest\nDefault Layout=us\nDefaultIM=pinyin\n\n'
                           '[Groups/0/Items/0]\nName=keyboard-us\n\n'
                           '[Groups/0/Items/1]\nName=pinyin\n\n[GroupOrder]\n0=LauncherTest\n')
                (fcitx_config / "profile").write_text(profile)
                (fcitx_config / "config").write_text('[Behavior]\nActiveByDefault=True\nShareInputState=All\n')
                (artifacts / "ime-profile.ini").write_text(profile)
                ime_log = (artifacts / "fcitx5.log").open("w")
                logs.append(ime_log)
                ime = subprocess.Popen(["fcitx5", "-D", "--disable=xcb,xim,ibus,cloudpinyin"], env=env,
                                       stdout=ime_log, stderr=subprocess.STDOUT, start_new_session=True)
                wait_for("private fcitx D-Bus service", lambda: "org.fcitx.Fcitx5" in run(
                    ["busctl", "--address=" + env["DBUS_SESSION_BUS_ADDRESS"], "--no-pager", "list"], env), ime)
                trace_path = artifacts / "ime.wayland.log"
                trace_file = trace_path.open("w")
                logs.append(trace_file)
                launcher = subprocess.Popen([str(launcher_binary), "--spike", "--output", "winit",
                                             "--quit-after", "10"], env=env | {"WAYLAND_DEBUG": "client"},
                                            stdout=trace_file, stderr=subprocess.STDOUT, start_new_session=True)
                wait_for("IME launcher layer", lambda: any(layer["namespace"] == NAMESPACE
                         for layer in ipc_json("layers")), launcher)
                time.sleep(0.3)
                # These commands can only reach the newly started Fcitx on the
                # private bus; input uses the private Wayland display above.
                run(["fcitx5-remote", "-s", "pinyin"], env)
                run(["fcitx5-remote", "-o"], env)
                run([str(keyboard), "49", "23", "35", "30", "24"], keyboard_env)
                time.sleep(0.3)
                run(["grim", "-o", "winit", str(artifacts / "ime-preedit.png")], env)
                run([str(keyboard), "57"], keyboard_env)
                time.sleep(0.15)
                run([str(keyboard), "+29", "30", "46", "-29"], keyboard_env)
                committed = run(["wl-paste", "--no-newline"], env, timeout=2)
                require(committed == "你好", f"Chinese commit mismatch: {committed!r}")
                run(["grim", "-o", "winit", str(artifacts / "ime-committed.png")], env)
                run([str(keyboard), "1"], keyboard_env)
                require(launcher.wait(timeout=3) == 0, "IME launcher Escape exit failed")
                launcher = None
                trace_file.flush()
                trace = trace_path.read_text(errors="replace")
                preedit = re.search(r'zwp_text_input_v3[@#]\d+\.preedit_string\("[^"\n]+"', trace)
                require(preedit, "No real text-input-v3 preedit received")
                rectangle = re.search(r'zwp_text_input_v3[@#]\d+\.set_cursor_rectangle\((\d+), (\d+), (\d+), (\d+)\)', trace)
                require(rectangle and rectangle.start() < preedit.start(),
                        "IME cursor rectangle was not published before composition")
                cursor_rectangle = list(map(int, rectangle.groups()))
                require(cursor_rectangle[0] > 0 and cursor_rectangle[1] > 0 and cursor_rectangle[3] > 0,
                        f"IME cursor rectangle is not in the input field: {cursor_rectangle}")
                require(re.search(r'zwp_text_input_v3[@#]\d+\.commit_string\("你好"\)', trace),
                        "No real text-input-v3 Chinese commit received")
                require(not any(layer["namespace"] == NAMESPACE for layer in ipc_json("layers")),
                        "IME cycle left a layer surface")
                report["ime"] = {"engine": "fcitx5 Pinyin", "private_bus": True,
                                 "committed_text_round_trip": committed,
                                 "native_keyboard_route": "fixed US virtual keyboard -> outer Clarion -> inner winit -> fcitx grab",
                                 "preedit_protocol_event": True, "commit_protocol_event": True,
                                 "cursor_rectangle_before_preedit": cursor_rectangle,
                                 "candidate_screenshot": "ime-preedit.png", "layer_removed": True}
            print("Testing same-process last-window drop/reopen lifecycle.", flush=True)
            lifecycle_path = artifacts / "lifecycle.wayland.log"
            lifecycle_log = lifecycle_path.open("w")
            logs.append(lifecycle_log)
            launcher = subprocess.Popen([str(launcher_binary), "--spike", "--lifecycle-probe",
                                         "--output", "winit", "--quit-after", "1"],
                                        env=env | {"WAYLAND_DEBUG": "client"}, stdout=lifecycle_log,
                                        stderr=subprocess.STDOUT, start_new_session=True)
            require(launcher.wait(timeout=6) == 0, "Lifecycle probe did not exit successfully")
            launcher = None
            lifecycle_log.flush()
            trace = lifecycle_path.read_text(errors="replace")
            roles = len(re.findall(r"zwlr_layer_shell_v1[@#]\d+\.get_layer_surface\(", trace))
            drops = trace.count("launcher-spike: last window dropped, cycle ")
            destroys = len(re.findall(r"zwlr_layer_surface_v1[@#]\d+\.destroy\(\)", trace))
            require((roles, drops, destroys) == (3, 2, 3),
                    f"Expected 3 mappings, 2 drops, 3 destroys; got {(roles, drops, destroys)}")
            require(".get_xdg_surface(" not in trace and ".get_toplevel(" not in trace,
                    "Lifecycle probe acquired an xdg role")
            require(not any(layer["namespace"] == NAMESPACE for layer in ipc_json("layers")),
                    "Lifecycle probe left a layer")
            report["same_process_lifecycle"] = {"exit_code": 0, "layer_roles": roles,
                                               "last_window_drops": drops,
                                               "layer_destroys": destroys, "layer_removed": True}
            if args.shell_native_test:
                print("Testing native Flutter backdrop, Dock and GPUI focus handoff.", flush=True)
                stop(ime)
                ime = None
                stop(inner_compositor)
                inner_compositor = None
                env = outer_env.copy()
                run([str(clarion), "msg", "output", "winit", "scale", "1"], env)
                test_root = private / "shell-test"
                test_root.mkdir()
                captures = artifacts / "shell-native"
                captures.mkdir(exist_ok=True)
                env.update(CLARION_SHELL_ISOLATED_TEST="1",
                           CLARION_SHELL_PARENT_WAYLAND_DISPLAY=str(parent_display),
                           CLARION_SHELL_PARENT_SOCKET=os.environ.get("CLARION_SOCKET", "parent-session"),
                           CLARION_SHELL_TEST_ROOT=str(test_root), CLARION_SHELL_CAPTURE_DIR=str(captures),
                           CLARION_SHELL_CLARION_BIN=str(clarion), CLARION_LAUNCHER_BIN=str(launcher_binary),
                           CLARION_SHELL_WTYPE=shutil.which("wtype"), CLARION_SHELL_GRIM=shutil.which("grim"),
                           CLARION_SHELL_SWAYBG=shutil.which("swaybg") or "swaybg",
                           CLARION_LAUNCHER_TEST_OUTPUT="winit", GDK_BACKEND="wayland")
                env["CLARION_SHELL_POINTER_HELPER"] = str(pointer_helper(private, env))
                flutter = os.environ.get("CLARION_FLUTTER_BIN")
                if not flutter:
                    candidates = [Path.home() / ".cache/clarion/toolchains/flutter/bin/flutter",
                                  Path.home() / ".cache/realand/toolchains/flutter/bin/flutter"]
                    flutter = next((str(path) for path in candidates if path.is_file()), None)
                require(flutter, "Flutter SDK is not available")
                shell_log = (artifacts / "shell-native.log").open("w")
                logs.append(shell_log)
                shell_process = subprocess.Popen(
                    [flutter, "test", "-d", "linux", "--no-pub",
                     "integration_test/gpui_launcher_spike_native_test.dart"],
                    cwd=ROOT / "clarion-shell", env=env, stdout=shell_log,
                    stderr=subprocess.STDOUT, start_new_session=True)
                require(shell_process.wait(timeout=240) == 0, "Native Flutter test failed; inspect shell-native.log")
                shell_process = None
                evidence = captures / "gpui-launcher-spike-native-evidence.json"
                require(evidence.is_file(), "Native Flutter test did not write its evidence")
                report["shell_native"] = json.loads(evidence.read_text())
                require(report["shell_native"].get("passed") is True, "Native Flutter evidence did not pass")
                report["exclusions"].remove("Flutter backdrop")
                pointer = report["shell_native"].get("dockPointer")
                if isinstance(pointer, dict) and pointer.get("controllerPanel") == "launcher":
                    report["exclusions"].remove("Dock interaction")
            report["passed"] = True
    except (SmokeFailure, OSError, subprocess.SubprocessError, ValueError) as error:
        report["error"] = str(error)
        print(f"Native smoke failed: {error}", file=sys.stderr, flush=True)
    finally:
        stop(launcher)
        stop(shell_process)
        stop(ime)
        stop(inner_compositor)
        stop(compositor)
        report["cleanup"] = True
        for log in logs:
            log.close()
        write_json(artifacts / "result.json", report)
    print(f"Evidence: {artifacts / 'result.json'}", flush=True)
    return 0 if report["passed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path, required=True, help="Directory for logs, PNGs and result.json")
    parser.add_argument("--scale2", action="store_true", help="Repeat at scale 2 and record whether the panel fits")
    parser.add_argument("--ime", action="store_true", help="Test actual Chinese composition on private fcitx5 Pinyin")
    parser.add_argument("--shell-native-test", action="store_true", help="Run the Flutter native GPUI composition integration test")
    parser.add_argument("--private-session", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not args.private_session:
        require(shutil.which("dbus-run-session"), "Missing dbus-run-session")
        display = os.environ.get("WAYLAND_DISPLAY", "")
        require(display, "A parent Wayland display is required for the nested compositor")
        parent = Path(display)
        if not parent.is_absolute():
            parent = Path(os.environ["XDG_RUNTIME_DIR"]) / parent
        argv = ["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()),
                "--artifacts", str(args.artifacts.resolve()), "--private-session"]
        if args.scale2:
            argv.append("--scale2")
        if args.ime:
            argv.append("--ime")
        if args.shell_native_test:
            argv.append("--shell-native-test")
        with tempfile.TemporaryDirectory(prefix="clarion-launcher-private-bus-") as temporary:
            env = os.environ | {"CLARION_TEST_PARENT_WAYLAND": str(parent.resolve()),
                                "CLARION_TEST_PARENT_DBUS_ADDRESS": os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")}
            for key in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DBUS_SESSION_BUS_ADDRESS",
                        "NOTIFY_SOCKET", "WATCHDOG_PID", "WATCHDOG_USEC"):
                env.pop(key, None)
            # dbus-daemon starts before the compositor exists. Give activation
            # private paths immediately and no display until discovery completes.
            for variable, directory in (("XDG_RUNTIME_DIR", "runtime"), ("XDG_CONFIG_HOME", "config"),
                                        ("XDG_DATA_HOME", "data"), ("XDG_CACHE_HOME", "cache"),
                                        ("XDG_STATE_HOME", "state")):
                target = Path(temporary) / directory
                target.mkdir(mode=0o700)
                env[variable] = str(target)
            bus_config = Path(temporary) / "bus.conf"
            bus_config.write_text('<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen>'
                                  '<auth>EXTERNAL</auth><policy context="default">'
                                  '<allow send_destination="*" eavesdrop="true"/>'
                                  '<allow eavesdrop="true"/><allow own="*"/>'
                                  '</policy></busconfig>\n')
            # No service directories: GPUI's optional portal lookup must not
            # auto-start document portals/FUSE mounts during this native test.
            argv.insert(1, "--config-file=" + str(bus_config))
            bus_process = subprocess.Popen(argv, env=env, start_new_session=True)
            try:
                return bus_process.wait()
            finally:
                stop(bus_process)
    return exercise(args)


if __name__ == "__main__":
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    try:
        raise SystemExit(main())
    except SmokeFailure as error:
        print(error, file=sys.stderr)
        raise SystemExit(1)
