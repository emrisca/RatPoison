#!/usr/bin/env python3
"""Generate clean, flat two-color vector app & tray icons for RatPoison
from the project's Braille rat ASCII art.

Palette: strictly two flat solid colors (zero gradients, glows, grids, or borders):
  - Background squircle: #141414 (matte ink)
  - Foreground rat art:  #F4F3EE (warm off-white)
"""

from __future__ import annotations

import os
import shutil
import struct
import subprocess
import tempfile
import zlib
from pathlib import Path

import numpy as np

REPO_ROOT = Path("/Users/abelmora/Documents/Avalon/RatPoison")
ICONS_DIR = REPO_ROOT / "src-tauri" / "icons"
FAVICON_PATH = REPO_ROOT / "frontend" / "public" / "favicon.png"
PREVIEW_PATH = Path(
    "/Users/abelmora/.openmausbot/task-workspaces/"
    "c3eb9e84-6281-44ca-89c5-e2fbc53aa35c/"
    "8110f9e7-6d79-42e8-9c8c-363cdf686762/ratpoison-icon-preview.png"
)

# Strictly two flat colors
BG_HEX = "#141414"
FG_HEX = "#F4F3EE"
BG_RGB = np.array([0x14, 0x14, 0x14], dtype=np.float32)
FG_RGB = np.array([0xF4, 0xF3, 0xEE], dtype=np.float32)

# Vector primitives in original Braille dot space: x in [0, 91.4], y in [0, 47.0]
# Each item: ("cubic", p0, p1, p2, p3, weight_scale) or ("line", p0, p1, weight_scale)
RAT_PRIMITIVES = [
    # 1. Left (rear) rounded ear
    ("cubic", (13.5, 25.0), (8.2, 23.0), (6.9, 17.6), (8.1, 13.0), 1.0),
    ("cubic", (8.1, 13.0), (9.3, 10.1), (13.8, 9.7), (18.2, 11.2), 1.0),
    # 2. Right (front) larger overlapping ear
    ("cubic", (19.5, 22.0), (16.5, 17.5), (16.2, 11.5), (20.8, 8.2), 1.0),
    ("cubic", (20.8, 8.2), (25.5, 5.1), (34.4, 5.8), (37.8, 11.0), 1.0),
    ("cubic", (37.8, 11.0), (39.7, 14.8), (38.5, 20.2), (35.2, 24.0), 1.0),
    # 3. Arched domed back
    ("cubic", (39.2, 16.5), (43.8, 8.0), (52.0, 0.4), (60.8, 0.3), 1.0),
    ("cubic", (60.8, 0.3), (69.6, 0.2), (76.6, 8.2), (79.8, 19.2), 1.0),
    ("cubic", (79.8, 19.2), (80.8, 23.2), (80.8, 26.8), (80.3, 31.2), 1.0),
    # 4. Forehead & snout -> flat chin
    ("cubic", (13.5, 25.6), (9.8, 28.2), (4.8, 31.4), (0.8, 34.5), 1.0),
    ("line", (0.8, 34.5), (29.0, 34.5), 1.0),
    # 5. Belly line (slight natural gap after chin, true to the Braille art)
    ("line", (33.8, 33.8), (75.0, 33.2), 1.0),
    # 6. Rear haunch / tiny leg tuck
    ("cubic", (76.0, 31.1), (77.4, 29.2), (79.0, 29.5), (80.3, 31.2), 0.82),
    # 7. Curled tail outer contour
    ("cubic", (80.7, 27.6), (87.8, 28.1), (91.3, 31.4), (91.1, 37.0), 1.0),
    ("cubic", (91.1, 37.0), (90.9, 42.8), (86.4, 46.8), (79.5, 47.0), 1.0),
    ("line", (79.5, 47.0), (58.0, 47.0), 1.0),
    ("cubic", (58.0, 47.0), (51.0, 46.8), (45.2, 44.6), (41.2, 42.2), 1.0),
    # 8. Curled tail inner contour
    ("cubic", (80.3, 31.2), (84.0, 33.2), (84.6, 37.5), (82.0, 40.5), 1.0),
    ("cubic", (82.0, 40.5), (80.3, 42.0), (77.8, 42.2), (74.8, 42.2), 1.0),
    ("line", (74.8, 42.2), (41.2, 42.2), 1.0),
    # 9. Tail ring segments (slightly lighter stroke for crisp readability)
    ("line", (84.0, 31.8), (89.3, 30.8), 0.76),
    ("line", (83.9, 35.7), (89.6, 37.0), 0.76),
    ("line", (82.0, 40.6), (85.4, 44.5), 0.76),
    ("line", (75.6, 42.2), (75.2, 46.8), 0.76),
    ("line", (68.3, 42.2), (67.9, 46.8), 0.76),
    ("line", (59.8, 42.2), (59.4, 46.8), 0.76),
    ("line", (54.6, 42.2), (54.2, 46.2), 0.76),
    ("line", (49.3, 42.2), (48.9, 44.8), 0.76),
]

EYE_POS = (23.5, 27.8)
EYE_RADIUS_DOTS = 1.45  # crisp circular dot


def transform_point(pt: tuple[float, float], scale_x: float, scale_y: float, ox: float, oy: float) -> tuple[float, float]:
    return (ox + pt[0] * scale_x, oy + pt[1] * scale_y)


def sample_primitive(prim, scale_x: float, scale_y: float, ox: float, oy: float) -> tuple[np.ndarray, float]:
    kind = prim[0]
    if kind == "line":
        _, p0, p1, w = prim
        a = np.array(transform_point(p0, scale_x, scale_y, ox, oy), dtype=np.float32)
        b = np.array(transform_point(p1, scale_x, scale_y, ox, oy), dtype=np.float32)
        return np.stack([a, b], axis=0), w
    elif kind == "cubic":
        _, p0, p1, p2, p3, w = prim
        pts = [np.array(transform_point(p, scale_x, scale_y, ox, oy), dtype=np.float32) for p in (p0, p1, p2, p3)]
        t = np.linspace(0.0, 1.0, 40, dtype=np.float32)[:, None]
        curve = (
            ((1 - t) ** 3) * pts[0]
            + 3 * ((1 - t) ** 2) * t * pts[1]
            + 3 * (1 - t) * (t**2) * pts[2]
            + (t**3) * pts[3]
        )
        return curve, w
    raise ValueError(kind)


def compute_rat_coverage(
    size: int,
    scale_x: float,
    scale_y: float,
    ox: float,
    oy: float,
    base_stroke_px: float,
    include_tail_stripes: bool = True,
) -> np.ndarray:
    """Exact analytical distance-to-polyline coverage in [0, 1] with 1px smooth AA."""
    yy, xx = np.mgrid[0:size, 0:size].astype(np.float32)
    xx += 0.5
    yy += 0.5

    # Track signed distance (negative inside stroke, positive outside)
    min_sd = np.full((size, size), 1e6, dtype=np.float32)

    for prim in RAT_PRIMITIVES:
        w_scale = prim[-1]
        if not include_tail_stripes and w_scale < 0.8:
            continue
        poly, _ = sample_primitive(prim, scale_x, scale_y, ox, oy)
        half_w = 0.5 * base_stroke_px * w_scale

        # Bounding box cull for speed
        pad = half_w + 3.0
        x_min = max(0, int(np.floor(poly[:, 0].min() - pad)))
        x_max = min(size, int(np.ceil(poly[:, 0].max() + pad)))
        y_min = max(0, int(np.floor(poly[:, 1].min() - pad)))
        y_max = min(size, int(np.ceil(poly[:, 1].max() + pad)))
        if x_min >= x_max or y_min >= y_max:
            continue

        sub_x = xx[y_min:y_max, x_min:x_max]
        sub_y = yy[y_min:y_max, x_min:x_max]
        best_d2 = np.full(sub_x.shape, 1e12, dtype=np.float32)

        for i in range(len(poly) - 1):
            ax, ay = poly[i]
            bx, by = poly[i + 1]
            abx = bx - ax
            aby = by - ay
            ab2 = abx * abx + aby * aby
            if ab2 < 1e-8:
                d2 = (sub_x - ax) ** 2 + (sub_y - ay) ** 2
            else:
                t = ((sub_x - ax) * abx + (sub_y - ay) * aby) / ab2
                np.clip(t, 0.0, 1.0, out=t)
                px = ax + t * abx
                py = ay + t * aby
                d2 = (sub_x - px) ** 2 + (sub_y - py) ** 2
            np.minimum(best_d2, d2, out=best_d2)

        sd = np.sqrt(best_d2) - half_w
        np.minimum(min_sd[y_min:y_max, x_min:x_max], sd, out=min_sd[y_min:y_max, x_min:x_max])

    # Crisp circular eye dot
    ex, ey = transform_point(EYE_POS, scale_x, scale_y, ox, oy)
    er = max(EYE_RADIUS_DOTS * scale_x, 0.68 * base_stroke_px)
    eye_sd = np.sqrt((xx - ex) ** 2 + (yy - ey) ** 2) - er
    np.minimum(min_sd, eye_sd, out=min_sd)

    # Smooth 1px analytical antialiasing
    aa = 0.75
    cov = np.clip(0.5 - min_sd / (2.0 * aa), 0.0, 1.0)
    # Smoothstep for ultra-clean optical edges
    cov = cov * cov * (3.0 - 2.0 * cov)
    return cov


def compute_squircle_coverage(size: int, margin_frac: float = 0.0625, power: float = 5.0) -> np.ndarray:
    """Clean Apple-style flat superellipse squircle mask with analytical AA."""
    yy, xx = np.mgrid[0:size, 0:size].astype(np.float32)
    cx = size * 0.5
    cy = size * 0.5
    r = size * (0.5 - margin_frac)
    dx = np.abs(xx + 0.5 - cx) / r
    dy = np.abs(yy + 0.5 - cy) / r
    # Normalized superellipse distance approximation in pixels
    f = (dx**power + dy**power) ** (1.0 / power)
    sd = (f - 1.0) * r
    aa = 0.85
    cov = np.clip(0.5 - sd / (2.0 * aa), 0.0, 1.0)
    cov = cov * cov * (3.0 - 2.0 * cov)
    return cov


def render_app_icon(size: int) -> np.ndarray:
    """Render flat two-color app icon at exact target resolution."""
    bg_alpha = compute_squircle_coverage(size, margin_frac=0.0625, power=5.0)

    # Fit rat cleanly inside the 87.5% squircle with generous horizontal breathing room
    rat_w_dots = 91.4
    rat_h_dots = 47.0
    target_w = size * 0.72
    scale_x = target_w / rat_w_dots
    scale_y = scale_x * 1.07  # subtle 7% vertical fullness for balanced square icon
    ox = (size - rat_w_dots * scale_x) * 0.5
    # Slight upward optical nudge so bottom-heavy tail/chin feel dead-centered
    oy = (size - rat_h_dots * scale_y) * 0.5 - size * 0.012

    # Slightly bolder relative stroke at small icon sizes so 32x32 stays crisp
    stroke_frac = 0.0195 if size >= 256 else (0.0215 if size >= 128 else 0.0255)
    base_stroke = size * stroke_frac
    rat_cov = compute_rat_coverage(
        size, scale_x, scale_y, ox, oy, base_stroke, include_tail_stripes=(size >= 32)
    )

    rgba = np.zeros((size, size, 4), dtype=np.float32)
    # Composite FG over flat BG strictly within the squircle
    for c in range(3):
        rgba[:, :, c] = BG_RGB[c] * (1.0 - rat_cov) + FG_RGB[c] * rat_cov
    rgba[:, :, 3] = bg_alpha * 255.0

    # Zero RGB where fully transparent for clean PNG compression
    rgba[bg_alpha <= 1e-4, :3] = 0.0
    return np.clip(np.round(rgba), 0, 255).astype(np.uint8)


def render_tray_icon(size: int = 44) -> np.ndarray:
    """Render crisp monochrome white macOS menu-bar template tray icon (44x44 @2x)."""
    rat_w_dots = 91.4
    rat_h_dots = 47.0
    target_w = size * 0.88
    scale_x = target_w / rat_w_dots
    scale_y = scale_x * 1.08
    ox = (size - rat_w_dots * scale_x) * 0.5
    oy = (size - rat_h_dots * scale_y) * 0.5

    # Bold clean monoline stroke for 22pt menu bar legibility; omit tiny tail ticks
    rat_cov = compute_rat_coverage(
        size, scale_x, scale_y, ox, oy, base_stroke_px=size * 0.041, include_tail_stripes=False
    )

    rgba = np.zeros((size, size, 4), dtype=np.uint8)
    rgba[:, :, 0] = 255
    rgba[:, :, 1] = 255
    rgba[:, :, 2] = 255
    rgba[:, :, 3] = np.clip(np.round(rat_cov * 255.0), 0, 255).astype(np.uint8)
    rgba[rgba[:, :, 3] == 0, :3] = 0
    return rgba


def build_svg(size: int = 1024, tray: bool = False) -> str:
    """Export matching clean flat two-color (or monochrome tray) SVG master."""
    rat_w_dots = 91.4
    rat_h_dots = 47.0
    if tray:
        target_w = size * 0.88
        scale_x = target_w / rat_w_dots
        scale_y = scale_x * 1.08
        ox = (size - rat_w_dots * scale_x) * 0.5
        oy = (size - rat_h_dots * scale_y) * 0.5
        base_stroke = size * 0.041
        fg = "#FFFFFF"
    else:
        target_w = size * 0.72
        scale_x = target_w / rat_w_dots
        scale_y = scale_x * 1.07
        ox = (size - rat_w_dots * scale_x) * 0.5
        oy = (size - rat_h_dots * scale_y) * 0.5 - size * 0.012
        base_stroke = size * 0.0195
        fg = FG_HEX

    def fmt(p):
        x, y = transform_point(p, scale_x, scale_y, ox, oy)
        return f"{x:.2f},{y:.2f}"

    lines = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" width="{size}" height="{size}">',
    ]
    if not tray:
        # Clean Apple-style squircle background (strictly flat color)
        m = size * 0.0625
        s = size - 2 * m
        r = s * 0.225
        lines.append(f'  <rect x="{m:.1f}" y="{m:.1f}" width="{s:.1f}" height="{s:.1f}" rx="{r:.1f}" ry="{r:.1f}" fill="{BG_HEX}"/>')

    lines.append(f'  <g fill="none" stroke="{fg}" stroke-linecap="round" stroke-linejoin="round">')
    for prim in RAT_PRIMITIVES:
        w_scale = prim[-1]
        if tray and w_scale < 0.8:
            continue
        sw = base_stroke * w_scale
        if prim[0] == "line":
            _, p0, p1, _ = prim
            lines.append(f'    <path d="M {fmt(p0)} L {fmt(p1)}" stroke-width="{sw:.2f}"/>')
        else:
            _, p0, p1, p2, p3, _ = prim
            lines.append(f'    <path d="M {fmt(p0)} C {fmt(p1)} {fmt(p2)} {fmt(p3)}" stroke-width="{sw:.2f}"/>')

    ex, ey = transform_point(EYE_POS, scale_x, scale_y, ox, oy)
    er = max(EYE_RADIUS_DOTS * scale_x, 0.68 * base_stroke)
    lines.append(f'    <circle cx="{ex:.2f}" cy="{ey:.2f}" r="{er:.2f}" fill="{fg}" stroke="none"/>')
    lines.append("  </g>")
    lines.append("</svg>")
    return "\n".join(lines) + "\n"


def write_png(path: Path, rgba: np.ndarray) -> None:
    h, w, _ = rgba.shape
    raw = b"".join(b"\x00" + rgba[y].tobytes() for y in range(h))

    def chunk(tag: bytes, data: bytes) -> bytes:
        crc = zlib.crc32(tag + data) & 0xFFFFFFFF
        return struct.pack("!I", len(data)) + tag + data + struct.pack("!I", crc)

    ihdr = struct.pack("!IIBBBBB", w, h, 8, 6, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


def write_ico(path: Path, png_blobs: list[tuple[int, bytes]]) -> None:
    header = struct.pack("<HHH", 0, 1, len(png_blobs))
    offset = 6 + 16 * len(png_blobs)
    entries = []
    for sz, blob in png_blobs:
        dim = 0 if sz >= 256 else sz
        entries.append(struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(blob), offset))
        offset += len(blob)
    path.write_bytes(header + b"".join(entries) + b"".join(blob for _, blob in png_blobs))


def build_icns(out_icns: Path) -> None:
    tmp = Path(tempfile.mkdtemp(prefix="ratpoison_iconset_"))
    iconset = tmp / "icon.iconset"
    iconset.mkdir()
    try:
        entries = [
            ("icon_16x16.png", 16),
            ("icon_16x16@2x.png", 32),
            ("icon_32x32.png", 32),
            ("icon_32x32@2x.png", 64),
            ("icon_128x128.png", 128),
            ("icon_128x128@2x.png", 256),
            ("icon_256x256.png", 256),
            ("icon_256x256@2x.png", 512),
            ("icon_512x512.png", 512),
            ("icon_512x512@2x.png", 1024),
        ]
        cache: dict[int, np.ndarray] = {}
        for fname, sz in entries:
            if sz not in cache:
                cache[sz] = render_app_icon(sz)
            write_png(iconset / fname, cache[sz])
        subprocess.run(
            ["iconutil", "-c", "icns", str(iconset), "-o", str(out_icns)],
            check=True,
        )
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def main() -> None:
    ICONS_DIR.mkdir(parents=True, exist_ok=True)

    # 1. Clean SVG masters
    (ICONS_DIR / "icon.svg").write_text(build_svg(1024, tray=False), encoding="utf-8")
    (ICONS_DIR / "tray-icon.svg").write_text(build_svg(44, tray=True), encoding="utf-8")

    # 2. Tauri PNG icons
    sizes = {
        ICONS_DIR / "icon.png": 1024,
        ICONS_DIR / "32x32.png": 32,
        ICONS_DIR / "128x128.png": 128,
        ICONS_DIR / "128x128@2x.png": 256,
        FAVICON_PATH: 64,
        PREVIEW_PATH: 512,
    }
    rendered: dict[int, np.ndarray] = {}
    for path, sz in sizes.items():
        if sz not in rendered:
            rendered[sz] = render_app_icon(sz)
        write_png(path, rendered[sz])
        print(f"Wrote {path} ({sz}x{sz})")

    # 3. Tray icon
    tray_rgba = render_tray_icon(44)
    write_png(ICONS_DIR / "tray-icon.png", tray_rgba)
    print(f"Wrote {ICONS_DIR / 'tray-icon.png'} (44x44)")

    # 4. Windows multi-size ICO (16, 32, 48, 64, 128, 256)
    ico_blobs: list[tuple[int, bytes]] = []
    with tempfile.TemporaryDirectory() as td:
        for sz in (16, 32, 48, 64, 128, 256):
            if sz not in rendered:
                rendered[sz] = render_app_icon(sz)
            p = Path(td) / f"{sz}.png"
            write_png(p, rendered[sz])
            ico_blobs.append((sz, p.read_bytes()))
    write_ico(ICONS_DIR / "icon.ico", ico_blobs)
    print(f"Wrote {ICONS_DIR / 'icon.ico'}")

    # 5. macOS ICNS
    build_icns(ICONS_DIR / "icon.icns")
    print(f"Wrote {ICONS_DIR / 'icon.icns'}")

    # 6. Verify strict two-color cleanliness on 512px preview
    img = rendered[512]
    opaque = img[img[:, :, 3] == 255]
    bg_only = np.all(opaque[:, :3] == BG_RGB.astype(np.uint8), axis=1).sum()
    fg_only = np.all(opaque[:, :3] == FG_RGB.astype(np.uint8), axis=1).sum()
    aa_px = len(opaque) - bg_only - fg_only
    print(f"\nColor audit (512x512 opaque pixels): BG={bg_only}, FG={fg_only}, AA edge={aa_px} ({100*aa_px/len(opaque):.1f}%)")

    # 7. Visual ASCII inspection of the actual rendered 512x512 icon
    step_x, step_y = 6, 12
    print("\n--- RENDERED 512x512 ICON VISUAL CHECK ---")
    for y in range(0, 512, step_y):
        row = []
        for x in range(0, 512, step_x):
            r, g, b, a = img[y, x]
            if a < 128:
                row.append(" ")
            elif r > 140:
                row.append("█")
            elif r > 55:
                row.append("▒")
            else:
                row.append("·")
        print("".join(row))


if __name__ == "__main__":
    main()
