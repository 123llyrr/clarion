#!/usr/bin/env python3
"""Create a self-contained 40-second Realand product film.

The film is intentionally generated from primitives rather than external stock
footage, so it can be rebuilt offline and kept in sync with the project.
Run from the repository root:

    python3 scripts/create_promo.py

Outputs are written to promo/. Requires Pillow, NumPy, rsvg-convert and ffmpeg.
"""

from __future__ import annotations

import math
import subprocess
import wave
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "promo"
OUT.mkdir(exist_ok=True)

WIDTH, HEIGHT = 1280, 720
FPS = 24
DURATION = 40.0
SAMPLES = int(48_000 * DURATION)

FONT_DIR = Path("/usr/share/fonts/sarasa-gothic")
FONT_REGULAR = FONT_DIR / "Sarasa-Regular.ttc"
FONT_SEMIBOLD = FONT_DIR / "Sarasa-SemiBold.ttc"
FONT_BOLD = FONT_DIR / "Sarasa-Bold.ttc"
FONT_MONO = FONT_DIR / "SarasaMonoSC-Regular.ttf"
if not FONT_MONO.exists():
    FONT_MONO = FONT_REGULAR

CYAN = (93, 229, 244)
MAGENTA = (255, 76, 194)
VIOLET = (132, 108, 255)
LIME = (183, 245, 126)
WHITE = (241, 247, 255)
MUTED = (145, 160, 184)
PANEL = (19, 27, 42)


def font(size: int, weight: str = "regular") -> ImageFont.FreeTypeFont:
    path = {"regular": FONT_REGULAR, "semibold": FONT_SEMIBOLD, "bold": FONT_BOLD, "mono": FONT_MONO}[weight]
    return ImageFont.truetype(str(path), size)


def rgba(color, alpha=255):
    return (*color, alpha)


def clamp(value, low=0.0, high=1.0):
    return max(low, min(high, value))


def ease(value):
    value = clamp(value)
    return value * value * (3.0 - 2.0 * value)


def ease_out(value):
    value = clamp(value)
    return 1.0 - (1.0 - value) ** 3


def mix(a, b, amount):
    return tuple(int(x + (y - x) * amount) for x, y in zip(a, b))


def hsv(hue, saturation=0.82, value=1.0):
    import colorsys

    return tuple(int(v * 255) for v in colorsys.hsv_to_rgb((hue % 360) / 360.0, saturation, value))


def text(draw, xy, value, size, color=WHITE, weight="regular", anchor=None, spacing=4, stroke=0):
    draw.multiline_text(
        xy,
        value,
        font=font(size, weight),
        fill=rgba(color),
        anchor=anchor,
        spacing=spacing,
        stroke_width=stroke,
    )


def line_text(draw, xy, value, size, color=WHITE, weight="regular"):
    text(draw, xy, value, size, color, weight)


def rounded(draw, box, radius, fill, outline=None, width=1):
    draw.rounded_rectangle(box, radius=radius, fill=fill, outline=outline, width=width)


def pill(draw, x, y, label, color=CYAN, width=None):
    f = font(15, "semibold")
    bounds = draw.textbbox((0, 0), label, font=f)
    w = width or (bounds[2] - bounds[0] + 28)
    rounded(draw, (x, y, x + w, y + 32), 16, rgba(color, 35), rgba(color, 170), 1)
    text(draw, (x + 14, y + 7), label, 15, color, "semibold")
    return w


def glow(image, center, radius, color, alpha=100):
    layer = Image.new("RGBA", (WIDTH // 5, HEIGHT // 5), (0, 0, 0, 0))
    draw = ImageDraw.Draw(layer)
    cx, cy = int(center[0] / 5), int(center[1] / 5)
    r = max(3, int(radius / 5))
    draw.ellipse((cx - r, cy - r, cx + r, cy + r), fill=rgba(color, alpha))
    layer = layer.filter(ImageFilter.GaussianBlur(max(4, r // 2)))
    image.alpha_composite(layer.resize((WIDTH, HEIGHT), Image.Resampling.BILINEAR))


def grid(draw, step=48, offset=0, alpha=28):
    for x in range(-step + int(offset) % step, WIDTH + step, step):
        draw.line((x, 0, x, HEIGHT), fill=(95, 130, 171, alpha), width=1)
    for y in range(0, HEIGHT, step):
        draw.line((0, y, WIDTH, y), fill=(95, 130, 171, alpha), width=1)


def base_background(scene, phase):
    """Precomputed-looking deep gradient with enough movement to feel alive."""
    y, x = np.mgrid[0:HEIGHT, 0:WIDTH]
    if scene == 0:
        top, bottom = np.array([8, 14, 28]), np.array([19, 10, 34])
    elif scene == 1:
        top, bottom = np.array([8, 17, 28]), np.array([11, 11, 27])
    elif scene == 2:
        top, bottom = np.array([18, 10, 35]), np.array([8, 15, 29])
    elif scene == 3:
        top, bottom = np.array([7, 17, 29]), np.array([12, 10, 28])
    elif scene == 4:
        top, bottom = np.array([7, 13, 23]), np.array([8, 12, 24])
    else:
        top, bottom = np.array([7, 10, 22]), np.array([21, 8, 32])
    amount = y[..., None] / HEIGHT
    arr = top * (1 - amount) + bottom * amount
    wave = (np.sin(x / 190 + phase * 0.7) + np.cos(y / 123 - phase * 0.45)) * 2.0
    arr = np.clip(arr + wave[..., None], 0, 255).astype(np.uint8)
    return Image.fromarray(arr, "RGB").convert("RGBA")


def corner_mark(draw, x, y, color=CYAN, size=19):
    draw.line((x, y + size, x, y, x + size, y), fill=rgba(color, 190), width=2)
    draw.line((x + size * 0.42, y + size, x + size, y + size, x + size, y + size * 0.42), fill=rgba(color, 80), width=1)


def app_window(draw, box, title, accent=CYAN, kind="code", active=False, scale=1.0):
    x1, y1, x2, y2 = [int(v) for v in box]
    if scale < 1:
        cx, cy = (x1 + x2) / 2, (y1 + y2) / 2
        nw, nh = (x2 - x1) * scale, (y2 - y1) * scale
        x1, x2, y1, y2 = int(cx - nw / 2), int(cx + nw / 2), int(cy - nh / 2), int(cy + nh / 2)
    rounded(draw, (x1 + 7, y1 + 9, x2 + 7, y2 + 9), 12, (2, 4, 10, 125))
    rounded(draw, (x1, y1, x2, y2), 12, (24, 33, 51, 245), rgba(accent, 230 if active else 70), 2 if active else 1)
    draw.rectangle((x1, y1, x2, y1 + 30), fill=(31, 42, 62, 255))
    for i, c in enumerate(((255, 97, 118), (255, 202, 91), (90, 221, 151))):
        draw.ellipse((x1 + 12 + i * 13, y1 + 11, x1 + 19 + i * 13, y1 + 18), fill=rgba(c, 210))
    text(draw, (x1 + 63, y1 + 7), title, 12, (184, 198, 219), "semibold")
    if active:
        # A rotating multicolor focus ring sells the compositor's animated borders.
        for i in range(4):
            color = hsv((i * 90 + (y1 + x1) * 0.1) % 360, 0.72, 1)
            draw.line((x1 + 16 + i * 10, y2 - 2, x1 + 68 + i * 12, y2 - 2), fill=rgba(color, 210), width=2)
    ix, iy = x1 + 18, y1 + 48
    iw, ih = x2 - x1 - 36, y2 - y1 - 64
    if kind == "code":
        colors = [(117, 231, 255), (195, 142, 255), (241, 203, 116), (151, 219, 142)]
        for row in range(max(3, int(ih / 19))):
            indent = (row % 4) * 11
            ln = f"{row + 1:02d}"
            text(draw, (ix, iy + row * 19), ln, 11, (91, 112, 140), "mono")
            draw.line((ix + 28 + indent, iy + row * 19 + 7, ix + 55 + indent + (row * 31) % max(40, int(iw - 80)), iy + row * 19 + 7), fill=rgba(colors[row % len(colors)], 180), width=3)
    elif kind == "browser":
        rounded(draw, (ix, iy, x2 - 18, iy + 28), 7, (15, 23, 38, 255), rgba(CYAN, 50), 1)
        draw.ellipse((ix + 11, iy + 10, ix + 17, iy + 16), fill=rgba(CYAN, 210))
        for row in range(3):
            yy = iy + 54 + row * 38
            draw.rectangle((ix, yy, ix + int(iw * 0.33), yy + 22), fill=(41, 63, 91, 210))
            draw.line((ix + int(iw * 0.4), yy + 8, x2 - 25, yy + 8), fill=(128, 166, 202, 135), width=3)
            draw.line((ix + int(iw * 0.4), yy + 17, x2 - 52, yy + 17), fill=(79, 111, 148, 100), width=2)
    else:
        for row in range(max(3, int(ih / 24))):
            text(draw, (ix, iy + row * 24), "$ " if row == 0 else "  ", 12, (132, 220, 166), "mono")
            draw.line((ix + 20, iy + row * 24 + 8, ix + 84 + (row * 67) % max(50, int(iw - 100)), iy + row * 24 + 8), fill=rgba((125, 204, 169), 160), width=3)


def desktop_frame(image, x, y, width, height, scroll=0.0, focus=0, overview=False):
    x, y, width, height = int(x), int(y), int(width), int(height)
    draw = ImageDraw.Draw(image, "RGBA")
    rounded(draw, (x + 10, y + 15, x + width + 10, y + height + 15), 20, (1, 3, 8, 160))
    rounded(draw, (x, y, x + width, y + height), 20, (18, 27, 44, 255), (90, 120, 160, 150), 1)
    draw.rectangle((x, y, x + width, y + 42), fill=(25, 36, 57, 255))
    text(draw, (x + 20, y + 12), "realand", 14, (180, 198, 222), "semibold")
    text(draw, (x + width - 102, y + 12), "● 3 workspaces", 12, (112, 143, 178), "regular")
    wx, wy = x + 13, y + 49
    ww, wh = width - 26, height - 62
    draw.rectangle((wx, wy, wx + ww, wy + wh), fill=(10, 17, 29, 255))
    for gx in range(wx + 20, wx + ww, 46):
        draw.line((gx, wy, gx, wy + wh), fill=(55, 82, 115, 32), width=1)
    for gy in range(wy + 22, wy + wh, 46):
        draw.line((wx, gy, wx + ww, gy), fill=(55, 82, 115, 28), width=1)
    if overview:
        for index in range(6):
            col, row = index % 3, index // 3
            px = wx + 30 + col * (ww - 80) / 3
            py = wy + 24 + row * (wh - 52) / 2
            pw, ph = (ww - 110) / 3, (wh - 78) / 2
            active = index == focus
            rounded(draw, (px, py, px + pw, py + ph), 12, (23, 35, 56, 235), rgba(CYAN if active else (99, 119, 148), 220 if active else 80), 2 if active else 1)
            text(draw, (px + 15, py + 12), f"workspace {index + 1}", 11, CYAN if active else (131, 150, 178), "semibold")
            for w in range(2 + index % 2):
                bx = px + 15 + (w % 2) * pw * 0.46
                by = py + 42 + (w // 2) * ph * 0.34
                rounded(draw, (bx, by, bx + pw * 0.37, by + ph * 0.18), 5, (54, 72, 99, 230), rgba(MAGENTA if w == 1 else CYAN, 80), 1)
    else:
        # The strip is deliberately wider than the viewport: scrolling becomes visible.
        positions = [(-40, 80, 210, 205), (185, 80, 400, 205), (410, 80, 625, 205), (635, 80, 850, 205)]
        offset = scroll * 150
        for column, (lx, ly, rx, ry) in enumerate(positions):
            lx += wx - 10 - offset
            rx += wx - 10 - offset
            if rx < wx or lx > wx + ww:
                continue
            accent = CYAN if column == focus else (75, 104, 143)
            # Dwindle puzzle: one column contains 2D splits, the next remains a stack.
            if column % 2 == 0:
                midx = (lx + rx) // 2
                app_window(draw, (lx, ly + wy, midx - 5, ry + wy), "editor.rs", accent, "code", column == focus)
                app_window(draw, (midx + 5, ly + wy, rx, ry + wy), "terminal", accent, "terminal", False)
            else:
                app_window(draw, (lx, ly + wy, rx, ly + wy + 112), "notes", accent, "browser", column == focus)
                app_window(draw, (lx, ly + wy + 124, rx, ry + wy), "preview", accent, "code", False)
        # Horizontal scroll indicator and gesture hint.
        draw.line((wx + 30, wy + wh - 16, wx + ww - 30, wy + wh - 16), fill=(63, 87, 119, 180), width=2)
        handle_x = wx + 90 + (scroll % 1.0) * max(80, ww - 180)
        draw.rounded_rectangle((handle_x, wy + wh - 20, handle_x + 120, wy + wh - 12), radius=4, fill=rgba(CYAN, 190))
        text(draw, (wx + ww - 166, wy + wh - 40), "drag / scroll →", 11, (118, 150, 183), "mono")


def intro(image, local):
    draw = ImageDraw.Draw(image, "RGBA")
    p = ease_out(local / 5.0)
    grid(draw, 56, local * 14, 18)
    glow(image, (180 + local * 35, 160), 330, CYAN, 90)
    glow(image, (1030 - local * 26, 590), 300, MAGENTA, 70)
    # Orbiting points suggest the logo's spatial/desktop idea.
    cx, cy = 640, 300
    for i in range(18):
        angle = local * 0.8 + i * math.tau / 18
        rad = 130 + 26 * math.sin(i * 1.7)
        px = cx + math.cos(angle) * rad
        py = cy + math.sin(angle) * rad * 0.56
        r = 2 + (i % 3)
        draw.ellipse((px - r, py - r, px + r, py + r), fill=rgba(hsv(i * 20 + local * 30, 0.7, 1), 190),)
    logo = load_logo()
    size = int(178 + 18 * math.sin(local * 2.2))
    logo_copy = logo.copy()
    logo_copy.thumbnail((size, size), Image.Resampling.LANCZOS)
    image.alpha_composite(logo_copy, (cx - logo_copy.width // 2, 118))
    alpha = int(255 * clamp((local - 0.25) / 0.8))
    text(draw, (640, 350), "REALAND", 60, WHITE, "bold", anchor="mm")
    text(draw, (640, 422), "让桌面，顺着你的思路流动", 27, CYAN, "semibold", anchor="mm")
    text(draw, (640, 625), "A modern Wayland compositor for focused work", 14, (159, 178, 207), "regular", anchor="mm")
    # Thin signal line.
    line_x = 485 + 180 * p
    draw.line((485, 486, line_x, 486), fill=rgba(CYAN, 210), width=2)
    draw.ellipse((line_x - 4, 482, line_x + 4, 490), fill=rgba(WHITE, 235))


def tiling(image, local):
    draw = ImageDraw.Draw(image, "RGBA")
    grid(draw, 56, local * 22, 17)
    glow(image, (1040, 170 + local * 28), 330, CYAN, 72)
    glow(image, (180, 630 - local * 18), 320, VIOLET, 62)
    pill(draw, 70, 72, "01  DUAL TILING", CYAN)
    text(draw, (70, 140), "无限滚动\n与动态切分", 47, WHITE, "bold", spacing=2)
    text(draw, (72, 276), "同一个工作区，自由切换两种思路。", 20, (183, 199, 222), "regular")
    text(draw, (72, 329), "SCROLLING TILING  ×  DWINDLE", 15, CYAN, "mono")
    pill(draw, 70, 580, "Mod + Shift + D", VIOLET)
    text(draw, (72, 634), "columns keep moving — your focus stays", 13, (126, 151, 184), "mono")
    frame_x = 430
    # Entering the frame and then sliding across the strip gives the shot a clear motion arc.
    enter = ease_out(clamp(local / 1.3))
    desktop_frame(image, frame_x + (1 - enter) * 90, 105, 790, 505, scroll=local * 0.38, focus=int(local * 0.55) % 4)
    draw = ImageDraw.Draw(image, "RGBA")
    text(draw, (847, 642), "one workspace / many ways to think", 13, (139, 163, 194), "regular", anchor="mm")


def motion(image, local):
    draw = ImageDraw.Draw(image, "RGBA")
    grid(draw, 56, local * 18, 15)
    glow(image, (940, 200), 350, MAGENTA, 78)
    glow(image, (270, 550), 280, CYAN, 60)
    pill(draw, 70, 72, "02  MOTION WITH INTENT", MAGENTA)
    text(draw, (70, 145), "每一次操作，\n都有回应。", 45, WHITE, "bold", spacing=2)
    text(draw, (72, 286), "Spring physics · Popin · Gradient focus ring", 15, (231, 157, 220), "mono")
    text(draw, (72, 344), "弹性开窗、平滑重排、流动边框。", 20, (190, 203, 223), "regular")
    code = "animations {\n  window-open { popin 0.8 }\n  window-movement { spring }\n}"
    rounded(draw, (70, 454, 360, 590), 12, (15, 23, 38, 225), (255, 100, 205, 90), 1)
    text(draw, (91, 475), code, 13, (172, 214, 232), "mono", spacing=7)
    # A UI composition that changes from one large surface into a dynamic split.
    x, y, w, h = 494, 120, 715, 485
    rounded(draw, (x + 10, y + 14, x + w + 10, y + h + 14), 20, (1, 3, 8, 150))
    rounded(draw, (x, y, x + w, y + h), 20, (14, 22, 37, 252), (100, 119, 163, 170), 1)
    draw.rectangle((x, y, x + w, y + 42), fill=(27, 36, 55, 255))
    text(draw, (x + 20, y + 12), "active workspace  /  dynamic reshape", 13, (181, 198, 220), "semibold")
    p = ease_out(clamp((local - 0.3) / 2.3))
    split = x + 350 + int(90 * p)
    app_window(draw, (x + 25, y + 68, split - 8, y + h - 28), "focus-ring.rs", CYAN, "code", True)
    second_scale = 0.72 + p * 0.28
    app_window(draw, (split + 8, y + 68, x + w - 25, y + h - 28), "wayland-client", MAGENTA, "browser", False, second_scale)
    angle = local * 180
    cx, cy = 840, 640
    for i in range(4):
        c = hsv(angle + i * 90, 0.8, 1)
        draw.arc((cx - 126 - i * 4, cy - 36 - i * 2, cx + 126 + i * 4, cy + 36 + i * 2), int(angle + i * 18), int(angle + 135 + i * 18), fill=rgba(c, 160), width=2)


def overview(image, local):
    draw = ImageDraw.Draw(image, "RGBA")
    grid(draw, 64, local * 11, 16)
    glow(image, (1000, 170), 330, VIOLET, 85)
    glow(image, (240, 590), 300, CYAN, 62)
    pill(draw, 70, 72, "03  OVERVIEW", VIOLET)
    text(draw, (70, 145), "一眼看见\n全部空间", 47, WHITE, "bold", spacing=2)
    text(draw, (72, 281), "工作区总览、拖拽整理、快速回到焦点。", 19, (190, 204, 225), "regular")
    text(draw, (72, 330), "EXPO GRID  /  NAVIGATE  /  ARRANGE", 14, (180, 161, 255), "mono")
    pill(draw, 70, 578, "Mod + E", CYAN)
    text(draw, (72, 634), "move windows across the wall", 13, (126, 151, 184), "mono")
    zoom = ease_out(clamp((local - 0.2) / 1.0))
    frame_w = 690 + int(55 * zoom)
    frame_h = 440 + int(32 * zoom)
    desktop_frame(image, 465 - int(28 * zoom), 135 - int(16 * zoom), frame_w, frame_h, focus=int(local * 1.4) % 6, overview=True)
    draw = ImageDraw.Draw(image, "RGBA")
    # A small pointer makes the drag-and-drop idea legible without needing captured footage.
    t = clamp((local - 2.0) / 2.0)
    sx, sy = 625, 298
    ex, ey = 980, 472
    px, py = sx + (ex - sx) * ease(t), sy + (ey - sy) * ease(t)
    draw.line((sx, sy, px, py), fill=rgba(CYAN, 125), width=2)
    draw.ellipse((px - 7, py - 7, px + 7, py + 7), fill=rgba(CYAN, 220), outline=rgba(WHITE, 235), width=2)
    draw.polygon([(px, py), (px + 5, py + 21), (px + 11, py + 12), (px + 20, py + 13)], fill=rgba(WHITE, 235))


def performance(image, local):
    draw = ImageDraw.Draw(image, "RGBA")
    grid(draw, 48, local * 7, 11)
    glow(image, (260, 210), 280, CYAN, 62)
    glow(image, (1080, 540), 300, MAGENTA, 55)
    pill(draw, 70, 72, "04  UNDER THE SURFACE", LIME)
    text(draw, (70, 145), "为每一帧，\n保留余量。", 47, WHITE, "bold", spacing=2)
    text(draw, (72, 282), "Rust  ·  Smithay  ·  Wayland  ·  Vulkan", 15, LIME, "mono")
    text(draw, (72, 335), "从输入、布局到呈现，性能意识贯穿全栈。", 19, (190, 204, 225), "regular")
    # Code / pipeline card.
    rounded(draw, (70, 450, 490, 612), 14, (13, 22, 34, 235), (154, 236, 132, 82), 1)
    text(draw, (94, 472), "render_pipeline.rs", 14, (187, 225, 182), "semibold")
    text(draw, (94, 507), "input  →  layout  →  shader  →  present", 14, (150, 183, 205), "mono")
    draw.line((96, 550, 453, 550), fill=rgba(LIME, 80), width=1)
    for i, label in enumerate(("INPUT", "LAYOUT", "GPU", "OUTPUT")):
        bx = 95 + i * 89
        draw.ellipse((bx, 540, bx + 20, 560), fill=rgba(LIME if i < 3 else CYAN, 210))
        text(draw, (bx - 4, 574), label, 10, (134, 161, 177), "mono")
        if i < 3:
            draw.line((bx + 22, 550, bx + 78, 550), fill=rgba(LIME, 120), width=2)
    # Live-looking frame chart.
    chart = (580, 170, 1195, 526)
    rounded(draw, chart, 16, (12, 20, 34, 235), (96, 128, 164, 95), 1)
    text(draw, (610, 198), "FRAME TIMINGS", 14, (170, 194, 219), "semibold")
    text(draw, (610, 226), "steady by design", 12, (114, 143, 177), "mono")
    cx, cy, cw, ch = 610, 278, 540, 185
    for yy in range(cy, cy + ch + 1, 46):
        draw.line((cx, yy, cx + cw, yy), fill=(76, 103, 138, 42), width=1)
    pts = []
    for i in range(86):
        xx = cx + i / 85 * cw
        signal = 0.46 + 0.08 * math.sin(i * 0.56 + local * 5) + 0.03 * math.sin(i * 1.7)
        if 42 < i < 51:
            signal += 0.12 * math.sin((i - 42) / 9 * math.pi)
        pts.append((xx, cy + signal * ch))
    draw.line(pts, fill=rgba(CYAN, 225), width=3)
    for i, (label, value, color) in enumerate((("KMS LATCH", "READY", CYAN), ("GPU SYNC", "NATIVE", LIME), ("RENDER PATH", "VULKAN", MAGENTA))):
        bx = 610 + i * 172
        rounded(draw, (bx, 487, bx + 154, 566), 10, (22, 32, 49, 220), rgba(color, 115), 1)
        text(draw, (bx + 14, 502), label, 10, (129, 155, 184), "mono")
        text(draw, (bx + 14, 525), value, 16, color, "semibold")


def outro(image, local):
    draw = ImageDraw.Draw(image, "RGBA")
    grid(draw, 56, local * 8, 16)
    glow(image, (640, 250), 420, CYAN, 80)
    glow(image, (1050, 560), 250, MAGENTA, 70)
    # Fade the orbit down into a clean end card.
    for i in range(14):
        angle = local * 0.9 + i * math.tau / 14
        rad = 170 + i * 2
        px, py = 640 + math.cos(angle) * rad, 258 + math.sin(angle) * rad * 0.5
        draw.ellipse((px - 2, py - 2, px + 2, py + 2), fill=rgba(hsv(i * 25 + local * 35, 0.7, 1), 190),)
    logo = load_logo()
    logo_copy = logo.copy()
    logo_copy.thumbnail((155, 155), Image.Resampling.LANCZOS)
    image.alpha_composite(logo_copy, (WIDTH // 2 - logo_copy.width // 2, 138))
    text(draw, (640, 342), "REALAND", 63, WHITE, "bold", anchor="mm")
    text(draw, (640, 415), "让桌面，顺着你的思路流动", 26, CYAN, "semibold", anchor="mm")
    text(draw, (640, 482), "A modern Wayland compositor", 16, (173, 192, 218), "regular", anchor="mm")
    text(draw, (640, 590), "SCROLL  ·  SPLIT  ·  CREATE", 14, (134, 159, 190), "mono", anchor="mm")
    draw.line((438, 626, 842, 626), fill=rgba(CYAN, 120), width=1)


LOGO: Image.Image | None = None


def load_logo():
    global LOGO
    if LOGO is None:
        logo_path = OUT / "realand-logo.png"
        svg_path = ROOT / "docs/wiki/_assets/icons/logo.svg"
        subprocess.run(["rsvg-convert", "-w", "512", "-h", "512", "-o", str(logo_path), str(svg_path)], check=True)
        LOGO = Image.open(logo_path).convert("RGBA")
    return LOGO


def render_frame(time):
    if time < 5:
        scene, local = 0, time
        render = intro
    elif time < 13:
        scene, local = 1, time - 5
        render = tiling
    elif time < 21:
        scene, local = 2, time - 13
        render = motion
    elif time < 29:
        scene, local = 3, time - 21
        render = overview
    elif time < 35:
        scene, local = 4, time - 29
        render = performance
    else:
        scene, local = 5, time - 35
        render = outro
    image = base_background(scene, time)
    render(image, local)
    # Short dip-to-color transitions separate the six beats like a finished edit.
    boundaries = (5, 13, 21, 29, 35)
    for boundary in boundaries:
        distance = abs(time - boundary)
        if distance < 0.16:
            overlay = Image.new("RGBA", (WIDTH, HEIGHT), (3, 7, 16, int(185 * (1 - distance / 0.16))))
            image.alpha_composite(overlay)
    return image.convert("RGB")


def make_audio(path: Path):
    """Make a restrained electronic bed and soft UI-like transition accents."""
    t = np.arange(SAMPLES, dtype=np.float32) / 48_000.0
    # A minor, slowly breathing pad.
    pad = (
        np.sin(2 * np.pi * 110.0 * t)
        + 0.72 * np.sin(2 * np.pi * 130.81 * t + 0.4)
        + 0.54 * np.sin(2 * np.pi * 164.81 * t + 1.2)
    ) / 3.0
    pad *= 0.07 * (0.72 + 0.28 * np.sin(2 * np.pi * 0.07 * t) ** 2)
    beat_phase = np.mod(t, 0.5)
    kick = np.exp(-beat_phase * 23) * np.sin(2 * np.pi * (52 + 15 * np.exp(-beat_phase * 18)) * beat_phase)
    kick *= (beat_phase < 0.18) * 0.13
    bass = np.sin(2 * np.pi * (55 + 4 * np.sin(2 * np.pi * 0.03 * t)) * t) * 0.045
    hat_phase = np.mod(t + 0.125, 0.25)
    hats = np.sin(2 * np.pi * 3400 * t) * np.exp(-hat_phase * 55) * (hat_phase < 0.045) * 0.025
    audio = pad + kick + bass + hats
    # Rising accents at every edit point.
    for mark in (5.0, 13.0, 21.0, 29.0, 35.0):
        q = t - mark
        safe_q = np.maximum(q, 0)
        sweep = (q >= 0) * (q < 0.8) * np.exp(-safe_q * 4.8) * np.sin(2 * np.pi * (260 + 880 * safe_q) * safe_q)
        audio += sweep * 0.075
    fade = np.minimum(1.0, t / 0.8) * np.minimum(1.0, (DURATION - t) / 0.8)
    audio *= fade
    audio = np.clip(audio, -0.45, 0.45)
    stereo = np.column_stack((audio * 0.97, audio * 0.91)).astype(np.float32)
    pcm = (stereo * 32767).astype(np.int16)
    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(2)
        wav.setsampwidth(2)
        wav.setframerate(48_000)
        wav.writeframes(pcm.tobytes())


def write_shotlist():
    (OUT / "shotlist.md").write_text(
        "# Realand 40s product film\n\n"
        "| Time | Beat | On-screen message |\n"
        "| --- | --- | --- |\n"
        "| 00–05 | Opening | 让桌面，顺着你的思路流动 |\n"
        "| 05–13 | Dual tiling | 无限滚动与动态切分，共存于同一个工作区 |\n"
        "| 13–21 | Motion | Spring physics · Popin · Gradient focus ring |\n"
        "| 21–29 | Overview | 一眼看见全部空间，拖拽整理窗口 |\n"
        "| 29–35 | Performance | Rust · Smithay · Wayland · Vulkan |\n"
        "| 35–40 | End card | Realand — A modern Wayland compositor |\n\n"
        "The visuals are generated from the project's documented features; no external stock media is required.\n",
        encoding="utf-8",
    )


def main():
    video = OUT / "realand-40s.mp4"
    audio = OUT / "realand-40s-audio.wav"
    write_shotlist()
    make_audio(audio)
    load_logo()
    cmd = [
        "ffmpeg", "-y", "-loglevel", "error",
        "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{WIDTH}x{HEIGHT}", "-r", str(FPS), "-i", "-",
        "-i", str(audio), "-t", str(DURATION),
        "-c:v", "libx264", "-preset", "medium", "-crf", "18", "-pix_fmt", "yuv420p",
        "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart", str(video),
    ]
    print(f"Rendering {WIDTH}x{HEIGHT} / {FPS}fps / {DURATION:.0f}s → {video}")
    process = subprocess.Popen(cmd, stdin=subprocess.PIPE)
    assert process.stdin is not None
    try:
        for frame_number in range(int(DURATION * FPS)):
            if frame_number % FPS == 0:
                print(f"  {frame_number / FPS:02.0f}s / {DURATION:.0f}s")
            frame = render_frame(frame_number / FPS)
            process.stdin.write(np.asarray(frame, dtype=np.uint8).tobytes())
    finally:
        process.stdin.close()
    return_code = process.wait()
    if return_code:
        raise SystemExit(return_code)
    print(f"Done: {video}")


if __name__ == "__main__":
    main()
