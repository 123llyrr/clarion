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
import struct
import subprocess
import sys
import tempfile
import time
import zlib
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[1]
MARKER = ".native-shell-smoke-owner"
FIXTURE_BAR_HEIGHT = 24
PREFERENCES_WINDOW_RULE = r'''window-rule {
    match app-id=r#"^io\.github\.clarion\.DesktopPreferences$"#
    open-floating true
    default-column-width { fixed 640; }
    default-window-height { fixed 520; }
    border { off; }
    focus-ring { off; }
    geometry-corner-radius 10
    clip-to-geometry true
}
'''


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


def ppm_center_sample(data):
    """Read grim's single binary P6 frame without treating RGB bytes as a header.

    Comments and ordinary header whitespace are accepted. The separator after
    maxval is consumed exactly once, so a pixel beginning with space/newline/#
    remains pixel data rather than being skipped as additional header text.
    """
    whitespace = b" \t\r\n\v\f"
    position = 0
    tokens = []
    for _ in range(4):
        while position < len(data) and position < 4096:
            if data[position] in whitespace:
                position += 1
            elif data[position] == ord('#'):
                newline = data.find(b"\n", position, 4096)
                require(newline != -1, "Malformed PPM header comment")
                position = newline + 1
            else:
                break
        start = position
        while position < len(data) and position < 4096 and data[position] not in whitespace:
            position += 1
        require(position > start and position < 4096, "Malformed or oversized PPM header")
        tokens.append(data[start:position])
    require(tokens[0] == b"P6", "grim did not return binary P6 RGB")
    require(all(token.isdigit() and len(token) <= 10 for token in tokens[1:]),
            "Malformed PPM dimensions or maxval")
    width, height, maxval = map(int, tokens[1:])
    require(0 < width <= 16384 and 0 < height <= 16384 and maxval == 255,
            "Unsupported PPM dimensions or sample depth")
    require(position < len(data) and data[position] in whitespace, "Missing PPM pixel separator")
    position += 2 if data[position:position + 2] == b"\r\n" else 1
    require(len(data) - position == width * height * 3, "Truncated or oversized PPM RGB frame")
    pixel = position + ((height // 2) * width + width // 2) * 3
    return {"width": width, "height": height, "center_rgb": list(data[pixel:pixel + 3])}


def diagnostic_wallpaper_visible(rgb):
    # This predicate identifies only our owned colorful diagnostic fixture. It
    # is not a claim that arbitrary wallpapers or Apple reference art match.
    return rgb[0] > 100 and max(rgb) - min(rgb) > 35


def wait_for_diagnostic_wallpaper(env, processes, timeout, evidence):
    started = time.monotonic()
    deadline = started + timeout
    evidence.update(ready=False, probe_count=0, samples=[],
                    scope="owned winit diagnostic pattern only; never parent output",
                    criterion="center R > 100 and RGB spread > 35")

    def visible():
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return False
        # The regular captures use a ten-second subprocess limit. Clip that
        # same maximum to the remaining, unchanged args.timeout readiness budget.
        command = ["grim", "-t", "ppm", "-o", "winit", "-"]
        evidence["probe_count"] += 1
        captured = subprocess.run(command, env=env, capture_output=True,
                                  timeout=min(10, remaining))
        require(captured.returncode == 0,
                f"Owned wallpaper pixel probe failed: {captured.stderr[-3000:]!r}")
        sample = ppm_center_sample(captured.stdout)
        sample["elapsed_seconds"] = round(time.monotonic() - started, 3)
        evidence["samples"] = (evidence["samples"] + [sample])[-32:]
        evidence["actual_center_rgb"] = sample["center_rgb"]
        return diagnostic_wallpaper_visible(sample["center_rgb"])

    try:
        wait_for("owned diagnostic wallpaper visible", visible, processes, timeout)
        evidence["ready"] = True
        evidence["ready_seconds"] = round(time.monotonic() - started, 3)
    finally:
        evidence["elapsed_seconds"] = round(time.monotonic() - started, 3)


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


def write_visual_wallpaper(path, width=1280, height=1280):
    """Original deterministic blur target, encoded without imaging packages.

    Broad diagonal colors expose unwanted opaque material; the high-frequency
    checks at both edges distinguish backdrop blur from a translucent overlay.
    This is a diagnostic pattern, not a macOS wallpaper or reference image.
    """
    palette = ((44, 97, 189), (66, 174, 183), (225, 199, 145),
               (224, 104, 118), (120, 81, 177), (44, 97, 189))
    color_row = []
    for x in range(width):
        scaled = x * (len(palette) - 1) / width
        index = min(int(scaled), len(palette) - 2)
        amount = scaled - index
        color_row.append(tuple(round(a + (b - a) * amount)
                               for a, b in zip(palette[index], palette[index + 1])))
    pixels = bytearray()
    for y in range(height):
        pixels.append(0)  # PNG row filter: None.
        edge = y < 96 or y >= height - 160
        for x in range(width):
            color = color_row[(x + y // 3) % width]
            texture = (1 if ((x // 8) + (y // 8)) % 2 else -1) * (30 if edge else 5)
            pixels.extend(max(0, min(255, channel + texture)) for channel in color)

    def chunk(kind, payload):
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(
            ">I", zlib.crc32(kind + payload) & 0xffffffff)

    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(
        b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)) + chunk(
        b"tEXt", b"Description\0Original Clarion backdrop-blur QA fixture; not Apple artwork") + chunk(
        b"IDAT", zlib.compress(pixels, 6)) + chunk(b"IEND", b""))


def seed_catalog(private, env, theme="dark", visual=False):
    """Use only disposable entries; none of their Exec commands are invoked."""
    applications = Path(env["XDG_DATA_HOME"]) / "applications"
    applications.mkdir(parents=True)
    icons = private / "fixture-icons"
    icons.mkdir()
    identifiers = []
    colors = ("#4f8ff0", "#b16ae5", "#56ad7e", "#db9a42", "#db6876")
    roles = ("safari.svg", "terminal.svg", "home.svg", "textedit.svg", "calculator.svg",
             "calendar.svg", "font-book.svg", "preview.png", "activity-monitor.png", "screenshot.png")
    for index in range(20):
        identifier = f"org.clarion.SmokeApp{index:02}.desktop"
        if visual:
            source = ROOT / "clarion-shell/assets/icons" / roles[index % len(roles)]
            require(source.is_file(), f"Missing open icon fixture: {source}")
            icon = icons / f"app-{index:02}{source.suffix}"
            shutil.copyfile(source, icon)
        else:
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
        {"theme": theme, "bar_height": FIXTURE_BAR_HEIGHT,
         "pinned_app_ids": identifiers[:6], "recent_app_ids": identifiers[6:9]},
        ensure_ascii=False) + "\n")
    if visual:
        wallpaper = private / "visual-wallpaper.png"
        write_visual_wallpaper(wallpaper)
        (preferences / "wallpaper.json").write_text(json.dumps(
            {"version": 1, "path": str(wallpaper)}, ensure_ascii=False) + "\n")
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


def negotiated_layer_geometry(trace, namespace, logical):
    """Combine actual server configure sizes with IPC output coordinates.

    Layers IPC currently reports roles, not rectangles. Keep that distinction
    explicit rather than claiming a requested WindowOptions size was displayed.
    """
    requests = list(re.finditer(
        r'get_layer_surface\(new id zwlr_layer_surface_v1[@#](\d+),[^\n]*"'
        + re.escape(namespace) + r'"\)', trace))
    if not requests:
        return None
    request = requests[-1]
    tail = trace[request.end():]
    name = r"zwlr_layer_surface_v1[@#]" + request.group(1)
    if re.search(name + r"\.destroy\(\)", tail):
        return None
    configured = re.findall(name + r"\.configure\(\d+,\s*(\d+),\s*(\d+)\)", tail)
    requested = re.findall(name + r"\.set_size\((\d+),\s*(\d+)\)", tail)
    anchors = re.findall(name + r"\.set_anchor\((\d+)\)", tail)
    margins = re.findall(name + r"\.set_margin\((-?\d+),\s*(-?\d+),\s*(-?\d+),\s*(-?\d+)\)", tail)
    zones = re.findall(name + r"\.set_exclusive_zone\((-?\d+)\)", tail)
    if not (configured and anchors and margins and zones):
        return None
    width, height = map(int, configured[-1])
    top, right, bottom, left = map(int, margins[-1])
    anchor = int(anchors[-1])
    output_width, output_height = logical["width"], logical["height"]
    x = (output_width - width + left - right) / 2
    y = (output_height - height + top - bottom) / 2
    if anchor & 4 and not anchor & 8:
        x = left
    elif anchor & 8 and not anchor & 4:
        x = output_width - right - width
    if anchor & 1 and not anchor & 2:
        y = top
    elif anchor & 2 and not anchor & 1:
        y = output_height - bottom - height
    return {"x": x, "y": y, "width": width, "height": height,
            "requested_size": list(map(int, requested[-1])) if requested else None,
            "surface_role_id": int(request.group(1)),
            "anchor": anchor, "margins": [top, right, bottom, left],
            "exclusive_zone": int(zones[-1]),
            "source": "compositor IPC Outputs.logical plus negotiated Wayland configure/anchors/margins"}


def preferences_window_geometry(window, logical, bar):
    layout = window["layout"]
    width = round(max(min(640, logical["width"] - 32), 280))
    height = round(max(min(520, logical["height"] - 48), 240))
    tile_position = layout.get("tile_pos_in_workspace_view")
    offset = layout.get("window_offset_in_tile", [0, 0])
    actual = ([tile_position[0] + offset[0], tile_position[1] + offset[1]]
              if tile_position is not None else None)
    # SetFixed is work-area-relative; the Shell compensates for that origin so
    # the actual window ends up centered in the full output's logical bounds.
    working_area_origin = [0, bar["height"]]
    expected = [(logical["width"] - width) / 2, (logical["height"] - height) / 2]
    return {"ready": (window.get("is_floating") is True
                      and layout["window_size"] == [width, height]
                      and actual is not None
                      and all(abs(a - b) <= 2 for a, b in zip(actual, expected))),
            "actual_window_origin": actual, "expected_window_origin": expected,
            "expected_window_size": [width, height], "working_area_origin": working_area_origin,
            "position_tolerance": 2,
            "source": "compositor IPC Windows.layout tile position plus window offset; output-local logical coordinates"}


def shell_surface_sizes_ready(logical, bar, desktop):
    if not (logical and bar and desktop):
        return False
    expected_bar = [logical["width"], FIXTURE_BAR_HEIGHT]
    expected_desktop = [logical["width"], logical["height"]]
    requested = desktop["requested_size"]
    desktop_size = [requested[index] if requested and requested[index] > 0 else desktop[key]
                    for index, key in enumerate(("width", "height"))]
    return (bar["requested_size"] == expected_bar
            and [bar["width"], bar["height"]] == expected_bar
            and desktop_size == expected_desktop
            and [desktop["width"], desktop["height"]] == expected_desktop)


def assert_component_geometry(command, actual, logical, bar, dock):
    width, height = logical["width"], logical["height"]
    top = bar["height"] + 8
    available = max(height - top - 12, 100)
    if command == "launcher":
        expected, anchor = [0, 0, width, height], 15
    elif command == "screenshot":
        tool_width = min(390, width - 24)
        # This fixture uses the default visible bottom Dock. Its canvas includes
        # 12 px of top shadow, leaving a further 4 px gap before the toolbar.
        require(dock is not None and dock["anchor"] == 2, "Fixture bottom Dock missing")
        bottom = dock["height"] + 12
        expected, anchor = [(width - tool_width) / 2, height - bottom - 64, tool_width, 64], 2
    else:
        preferred_width, panel_height = {
            "control-center": (320, min(424, available)),
            "notifications": (368, min(available, 1800)),
            "recent-content": (400, min(600, available)),
        }[command]
        panel_width = min(preferred_width, width - 24)
        expected, anchor = [width - panel_width - 12, top, panel_width, panel_height], 9
    observed = [actual[key] for key in ("x", "y", "width", "height")]
    require(actual["anchor"] == anchor and actual["exclusive_zone"] == -1
            and all(abs(a - b) <= 1 for a, b in zip(observed, expected)),
            f"{command} geometry mismatch: actual={actual}, expected={expected}, anchor={anchor}")
    return actual | {"expected_bounds": expected, "output_logical": logical}


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
        report["catalog_fixture_count"] = len(seed_catalog(private, env, args.theme, args.visual_fixture))
        report["theme"] = args.theme
        report["visual_fixture"] = args.visual_fixture
        if args.visual_fixture:
            shutil.copyfile(private / "visual-wallpaper.png", artifacts / "wallpaper-fixture.png")
            report["visual_fixture_scope"] = "original blur pattern and shipped open icons; not Apple reference evidence"
        config = private / "nested.kdl"
        config.write_text('input { keyboard { xkb { layout "us"; }; }; }\n'
                          'output "winit" { scale 1; }\nxwayland-satellite { off; }\n'
                          'hotkey-overlay { skip-at-startup; }\n'
                          'layer-rule { match namespace="^clarion-(shell|dock)$"; '
                          'background-effect { liquid-glass false; }; }\n'
                          + PREFERENCES_WINDOW_RULE)
        report["preferences_window_rule"] = {
            "app_id": "io.github.clarion.DesktopPreferences", "config": PREFERENCES_WINDOW_RULE,
            "scope": "explicit owned fixture rule for initial floating/size and native frame; IPC placement also verified"}
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

        def preferences_windows():
            # A normal xdg_toplevel belongs to this exact owned Shell process;
            # an app_id alone must not mistake another Settings app for it.
            return [window for window in ipc("windows")
                    if window.get("app_id") == "io.github.clarion.DesktopPreferences" and window.get("pid") == shell.pid]

        def send(command):
            run([sys.executable, str(ROOT / "resources/clarion-shell-msg"), command], env)

        confirmed_desktop = None
        report["surface_size_readiness"] = {}

        def align_shell_surfaces(phase):
            nonlocal confirmed_desktop
            evidence = report["surface_size_readiness"].setdefault(phase, {"ready": False})
            deadline = time.monotonic() + args.timeout

            def aligned():
                logical = ipc("outputs")["winit"]["logical"]
                if not logical:
                    return None
                trace = (artifacts / "shell.log").read_text()
                bar_geometry = negotiated_layer_geometry(trace, "clarion-shell", logical)
                desktop_geometry = negotiated_layer_geometry(trace, "clarion-desktop", logical)
                if not (bar_geometry and desktop_geometry):
                    evidence.update(output_logical=logical, bar=bar_geometry, desktop=desktop_geometry)
                    return None
                desktop_request = desktop_geometry["requested_size"]
                desktop_size = [
                    desktop_request[index] if desktop_request and desktop_request[index] > 0
                    else desktop_geometry[key]
                    for index, key in enumerate(("width", "height"))]
                evidence.update(output_logical=logical, bar=bar_geometry, desktop=desktop_geometry,
                                effective_desktop_size=desktop_size,
                                required_bar_size=[logical["width"], FIXTURE_BAR_HEIGHT])
                if not shell_surface_sizes_ready(logical, bar_geometry, desktop_geometry):
                    return None
                fingerprint = (bar_geometry["surface_role_id"], desktop_geometry["surface_role_id"],
                               logical["width"], logical["height"],
                               logical.get("scale"), logical.get("transform"))
                return {"fingerprint": fingerprint, "logical": logical}

            while time.monotonic() < deadline:
                remaining = deadline - time.monotonic()
                current = wait_for(f"{phase} client bar/desktop size matches IPC output",
                                   aligned, (core, shell), remaining)
                fingerprint = current["fingerprint"]
                if args.capture and args.visual_fixture and fingerprint != confirmed_desktop:
                    # Recreated desktop surfaces reload images asynchronously.
                    # Keep this pixel check inside the same readiness deadline.
                    wallpaper = {}
                    evidence.setdefault("wallpaper_checks", []).append(wallpaper)
                    wait_for_diagnostic_wallpaper(env, (core, shell),
                                                  max(0., deadline - time.monotonic()), wallpaper)
                    after = aligned()
                    if not after or after["fingerprint"] != fingerprint:
                        continue
                confirmed_desktop = fingerprint
                evidence["ready"] = True
                return current
            raise RuntimeError(f"Timed out: {phase} stable client geometry and diagnostic wallpaper")

        if args.capture and args.visual_fixture:
            report["wallpaper_readiness"] = {}
            wait_for_diagnostic_wallpaper(env, (core, shell), args.timeout,
                                          report["wallpaper_readiness"])
            report["checks"]["diagnostic_wallpaper_visible"] = True
        align_shell_surfaces("first-desktop")
        report["panel_timings"] = {}
        report["panel_geometry"] = {}
        if args.capture:
            run(["grim", "-o", "winit", str(artifacts / "desktop.png")], env)
        for command in ("launcher", "control-center", "notifications", "screenshot", "preferences", "recent-content"):
            align_shell_surfaces(f"before-{command}")
            started = time.monotonic()
            trace_offset = len((artifacts / "shell.log").read_text())
            send(command)
            observed = []
            def configured_panel():
                nonlocal observed
                if command == "preferences":
                    observed = preferences_windows()
                    workspaces = {workspace["id"]: workspace for workspace in ipc("workspaces")}
                    logical = ipc("outputs")["winit"]["logical"]
                    bar_geometry = negotiated_layer_geometry(
                        (artifacts / "shell.log").read_text(), "clarion-shell", logical)
                    geometry = (preferences_window_geometry(observed[0], logical, bar_geometry)
                                if len(observed) == 1 and bar_geometry else None)
                    report["preferences_geometry"] = geometry
                    return observed if (len(observed) == 1 and not panel()
                                        and workspaces.get(observed[0].get("workspace_id"), {}).get("output") == "winit"
                                        and observed[0].get("is_focused") is True
                                        and not observed[0].get("is_minimized", False)
                                        and geometry is not None and geometry["ready"]) else None
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
            if command == "preferences":
                report["preferences_window"] = layers[0]
            else:
                logical = ipc("outputs")["winit"]["logical"]
                geometry = wait_for(
                    f"{command} negotiated geometry",
                    lambda: negotiated_layer_geometry(
                        (artifacts / "shell.log").read_text()[trace_offset:], "clarion-panel", logical),
                    (core, shell), args.timeout)
                trace = (artifacts / "shell.log").read_text()
                bar_geometry = negotiated_layer_geometry(trace, "clarion-shell", logical)
                require(bar_geometry is not None, "Bar geometry evidence missing")
                dock_geometry = negotiated_layer_geometry(trace, "clarion-dock", logical)
                report["panel_geometry"][command] = assert_component_geometry(
                    command, geometry, logical, bar_geometry, dock_geometry)
            if args.capture:
                if command == "preferences":
                    # The IPC reports the committed target geometry while a
                    # compositor resize snapshot may still be on the screen.
                    # Present one owned frame before taking the static UI shot.
                    started = time.monotonic()
                    frame = subprocess.run(["grim", "-t", "ppm", "-o", "winit", "-"],
                                           env=env, capture_output=True, timeout=10)
                    require(frame.returncode == 0, "Owned preferences presentation barrier failed")
                    ppm_center_sample(frame.stdout)
                    timing["presentation_barrier_seconds"] = round(time.monotonic() - started, 3)
                started = time.monotonic()
                run(["grim", "-o", "winit", str(artifacts / f"{command}.png")], env)
                timing["capture_seconds"] = round(time.monotonic() - started, 3)
            started = time.monotonic()
            send("close")
            if command == "preferences":
                wait_for(f"{command} closed", lambda: not preferences_windows() and not panel(),
                         (core, shell), args.timeout)
            else:
                wait_for(f"{command} closed", lambda: not panel(), (core, shell), args.timeout)
            timing["closed_seconds"] = round(time.monotonic() - started, 3)
            report["panel_timings"][command] = timing
            report["checks"][command] = True
            require_no_legacy_launcher({core.pid, shell.pid})
        align_shell_surfaces("after-panels-desktop")
        if args.capture:
            run(["grim", "-o", "winit", str(artifacts / "desktop.png")], env)
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
    parser.add_argument("--theme", choices=("dark", "light"), default="dark")
    parser.add_argument("--timeout", type=int, default=20)
    parser.add_argument("--capture", action="store_true", help="Capture owned nested panels with grim")
    parser.add_argument("--visual-fixture", action="store_true",
                        help="Seed owned diagnostic wallpaper and shipped open app icons for visual QA")
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
                   "--artifacts", str(args.artifacts), "--renderer", args.renderer, "--theme", args.theme,
                   "--timeout", str(args.timeout)]
        if args.capture:
            command.append("--capture")
        if args.visual_fixture:
            command.append("--visual-fixture")
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
