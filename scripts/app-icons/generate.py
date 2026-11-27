#!/usr/bin/env python3
"""Generate gTerm application icons for Windows (.ico), macOS (.icns), Linux, and Slint UI.

Outputs:
  - assets/icons/svg/{gterm,gterm-ui,gterm-session-worker,gterm-mcp-daemon}.svg
  - assets/icons/{gterm,gterm-ui,gterm-session-worker,gterm-mcp-daemon}.ico
  - assets/icons/gterm.icns
  - assets/icons/linux/{16x16,32x32,48x48,128x128,256x256,512x512}/gterm.png
  - ui/icons/brand/gterm-{32,64,96,256}.png

Uses Python 3 standard library only (no Pillow/cairosvg dependency).
"""

from __future__ import annotations

import argparse
import math
import struct
import zlib
from pathlib import Path
from typing import Callable

ROOT = Path(__file__).resolve().parents[2]

Color = tuple[float, float, float, float]  # premultiplied or straight RGBA in 0..1


def hex_rgba(h: str, alpha: float = 1.0) -> Color:
    h = h.lstrip("#")
    r = int(h[0:2], 16) / 255.0
    g = int(h[2:4], 16) / 255.0
    b = int(h[4:6], 16) / 255.0
    if len(h) == 8:
        alpha *= int(h[6:8], 16) / 255.0
    return (r, g, b, alpha)


def clamp(v: float, lo: float = 0.0, hi: float = 1.0) -> float:
    return lo if v < lo else (hi if v > hi else v)


def lerp(a: float, b: float, t: float) -> float:
    return a + (b - a) * t


def lerp_color(c0: Color, c1: Color, t: float) -> Color:
    t = clamp(t)
    return (
        lerp(c0[0], c1[0], t),
        lerp(c0[1], c1[1], t),
        lerp(c0[2], c1[2], t),
        lerp(c0[3], c1[3], t),
    )


def over(dst: Color, src: Color) -> Color:
    """Straight-alpha Porter-Duff Source-Over."""
    sa = src[3]
    if sa <= 0.0:
        return dst
    if sa >= 1.0:
        return src
    da = dst[3]
    oa = sa + da * (1.0 - sa)
    if oa <= 1e-6:
        return (0.0, 0.0, 0.0, 0.0)
    inv_oa = 1.0 / oa
    return (
        (src[0] * sa + dst[0] * da * (1.0 - sa)) * inv_oa,
        (src[1] * sa + dst[1] * da * (1.0 - sa)) * inv_oa,
        (src[2] * sa + dst[2] * da * (1.0 - sa)) * inv_oa,
        oa,
    )


def sdf_round_rect(px: float, py: float, x0: float, y0: float, x1: float, y1: float, r: float) -> float:
    cx = 0.5 * (x0 + x1)
    cy = 0.5 * (y0 + y1)
    hx = 0.5 * (x1 - x0) - r
    hy = 0.5 * (y1 - y0) - r
    dx = abs(px - cx) - hx
    dy = abs(py - cy) - hy
    qx = dx if dx > 0.0 else 0.0
    qy = dy if dy > 0.0 else 0.0
    outside = math.hypot(qx, qy)
    inside = min(max(dx, dy), 0.0)
    return outside + inside - r


def sdf_circle(px: float, py: float, cx: float, cy: float, r: float) -> float:
    return math.hypot(px - cx, py - cy) - r


def sdf_segment(px: float, py: float, ax: float, ay: float, bx: float, by: float, r: float) -> float:
    pax = px - ax
    pay = py - ay
    bax = bx - ax
    bay = by - ay
    denom = bax * bax + bay * bay
    h = clamp((pax * bax + pay * bay) / denom) if denom > 1e-9 else 0.0
    return math.hypot(pax - bax * h, pay - bay * h) - r


def cov(sd: float, aa: float) -> float:
    return clamp(0.5 - sd / aa)


# ---------------------------------------------------------------------------
# Color palette (WinUI 3 / Fluent 2 aligned)
# ---------------------------------------------------------------------------

TILE_TOP = hex_rgba("#1E293B")
TILE_MID = hex_rgba("#0F172A")
TILE_BOT = hex_rgba("#090E1A")
TILE_RIM_TOP = hex_rgba("#64748B", 0.55)
TILE_RIM_BOT = hex_rgba("#334155", 0.65)
HEADER_LINE = hex_rgba("#94A3B8", 0.22)
DOT_RED = hex_rgba("#F87171", 0.90)
DOT_AMBER = hex_rgba("#FBBF24", 0.90)
DOT_GREEN = hex_rgba("#34D399", 0.90)
PROMPT_TOP = hex_rgba("#7EDEFF")
PROMPT_BOT = hex_rgba("#0078D4")
BAR_LEFT = hex_rgba("#F8FAFC")
BAR_RIGHT = hex_rgba("#BAE6FD")
NODE_OUTER = hex_rgba("#38BDF8")
NODE_CORE = hex_rgba("#E0F2FE")

BASE_TOP = hex_rgba("#F8FAFC")
BASE_BOT = hex_rgba("#CBD5E1")
BASE_BORDER = hex_rgba("#64748B", 0.85)
BASE_HEADER = hex_rgba("#94A3B8", 0.45)
BASE_PANE = hex_rgba("#FFFFFF", 0.55)
BASE_INK = hex_rgba("#334155")
BASE_INK_MUTED = hex_rgba("#64748B")


def sample_gterm_tile(
    x: float,
    y: float,
    x0: float,
    y0: float,
    size: float,
    aa: float,
    badge_mode: bool = False,
) -> Color:
    """Sample the main gTerm icon inside square [x0, x0+size] x [y0, y0+size]."""
    u = (x - x0) / size
    v = (y - y0) / size
    if u < -0.05 or u > 1.05 or v < -0.05 or v > 1.05:
        return (0.0, 0.0, 0.0, 0.0)

    # Convert to canonical 0..256 space relative to tile (margin 16 in 256 space -> full tile 0..1)
    # Here x0..x0+size represents the tile itself!
    px = u * 224.0
    py = v * 224.0
    local_aa = max(aa * (224.0 / size), 0.35)

    r = 46.0 if not badge_mode else 48.0
    sd_tile = sdf_round_rect(px, py, 0.0, 0.0, 224.0, 224.0, r)
    tile_cov = cov(sd_tile, local_aa)
    if tile_cov <= 0.0:
        return (0.0, 0.0, 0.0, 0.0)

    # Background gradient
    t_bg = clamp(0.35 * u + 0.65 * v)
    bg = lerp_color(TILE_TOP, TILE_MID, clamp(t_bg * 1.35)) if t_bg < 0.6 else lerp_color(TILE_MID, TILE_BOT, (t_bg - 0.6) / 0.4)

    # Subtle top-left Fluent accent glow behind glyph
    glow_dist = math.hypot((px - 112.0) / 130.0, (py - 112.0) / 110.0)
    glow_t = clamp(1.0 - glow_dist) ** 2 * 0.14
    bg = over(bg, (0.22, 0.74, 0.97, glow_t))

    out = bg

    if not badge_mode:
        # Subtle window header bar & 3 indicator dots
        sd_sep = sdf_segment(px, py, 14.0, 50.0, 210.0, 50.0, 1.5)
        c_sep = cov(sd_sep, local_aa)
        if c_sep > 0.0:
            out = over(out, (HEADER_LINE[0], HEADER_LINE[1], HEADER_LINE[2], HEADER_LINE[3] * c_sep))

        for cx, col in ((34.0, DOT_RED), (54.0, DOT_AMBER), (74.0, DOT_GREEN)):
            c_dot = cov(sdf_circle(px, py, cx, 29.0, 5.5), local_aa)
            if c_dot > 0.0:
                out = over(out, (col[0], col[1], col[2], col[3] * c_dot))

    # Prompt chevron `>` and wired cursor endpoint `_●`
    if badge_mode:
        # Optical boost for ~8-16px badge readability
        chev_r = 16.0
        sd_top = sdf_segment(px, py, 44.0, 70.0, 102.0, 114.0, chev_r)
        sd_bot = sdf_segment(px, py, 44.0, 158.0, 102.0, 114.0, chev_r)
        sd_chev = min(sd_top, sd_bot)
        sd_bar = sdf_segment(px, py, 108.0, 154.0, 158.0, 154.0, 13.0)
        sd_node = sdf_circle(px, py, 174.0, 154.0, 21.0)
        core_r = 8.0
        node_cx, node_cy = 174.0, 154.0
    else:
        chev_r = 11.5
        sd_top = sdf_segment(px, py, 44.0, 92.0, 98.0, 134.0, chev_r)
        sd_bot = sdf_segment(px, py, 44.0, 176.0, 98.0, 134.0, chev_r)
        sd_chev = min(sd_top, sd_bot)
        sd_bar = sdf_segment(px, py, 108.0, 166.0, 158.0, 166.0, 10.0)
        sd_node = sdf_circle(px, py, 174.0, 166.0, 17.5)
        core_r = 7.0
        node_cx, node_cy = 174.0, 166.0

    c_chev = cov(sd_chev, local_aa)
    if c_chev > 0.0:
        t_p = clamp((py - 70.0) / 106.0)
        pcol = lerp_color(PROMPT_TOP, PROMPT_BOT, t_p * 0.72)
        out = over(out, (pcol[0], pcol[1], pcol[2], c_chev))

    c_bar = cov(sd_bar, local_aa)
    if c_bar > 0.0:
        t_b = clamp((px - 108.0) / 50.0)
        bcol = lerp_color(BAR_LEFT, BAR_RIGHT, t_b)
        out = over(out, (bcol[0], bcol[1], bcol[2], c_bar))

    c_node = cov(sd_node, local_aa)
    if c_node > 0.0:
        out = over(out, (NODE_OUTER[0], NODE_OUTER[1], NODE_OUTER[2], c_node))
        c_core = cov(sdf_circle(px, py, node_cx, node_cy, core_r), local_aa)
        if c_core > 0.0:
            out = over(out, (NODE_CORE[0], NODE_CORE[1], NODE_CORE[2], c_core))

    # Crisp rim border
    stroke_w = 5.5 if badge_mode else 3.2
    sd_rim = abs(sd_tile + stroke_w * 0.5) - stroke_w * 0.5
    c_rim = cov(sd_rim, local_aa)
    if c_rim > 0.0:
        rcol = lerp_color(TILE_RIM_TOP, TILE_RIM_BOT, clamp(v))
        out = over(out, (rcol[0], rcol[1], rcol[2], rcol[3] * c_rim))

    return (out[0], out[1], out[2], out[3] * tile_cov)


def sample_base_card(px: float, py: float, x0: float, y0: float, x1: float, y1: float, r: float, aa: float) -> Color:
    sd = sdf_round_rect(px, py, x0, y0, x1, y1, r)
    c = cov(sd, aa)
    if c <= 0.0:
        return (0.0, 0.0, 0.0, 0.0)
    t = clamp((py - y0) / (y1 - y0))
    out = lerp_color(BASE_TOP, BASE_BOT, t)
    sd_rim = abs(sd + 2.5) - 2.5
    c_rim = cov(sd_rim, aa)
    if c_rim > 0.0:
        out = over(out, (BASE_BORDER[0], BASE_BORDER[1], BASE_BORDER[2], BASE_BORDER[3] * c_rim))
    return (out[0], out[1], out[2], out[3] * c)


def sample_gterm(px: float, py: float, aa: float) -> Color:
    return sample_gterm_tile(px, py, 16.0, 16.0, 224.0, aa, badge_mode=False)


def apply_badge_overlay(base_col: Color, px: float, py: float, aa: float) -> Color:
    # Badge sits at [122, 246] x [122, 246] (size 124). Cutout collar radius +10 around badge.
    bx0, by0, bsize = 122.0, 122.0, 124.0
    br = 48.0 * (bsize / 224.0)
    collar = 9.5
    sd_cutout = sdf_round_rect(px, py, bx0 - collar, by0 - collar, bx0 + bsize + collar, by0 + bsize + collar, br + collar)
    keep_base = clamp(sd_cutout / aa + 0.5)
    clipped_base = (base_col[0], base_col[1], base_col[2], base_col[3] * keep_base)
    badge = sample_gterm_tile(px, py, bx0, by0, bsize, aa, badge_mode=True)
    return over(clipped_base, badge)


def sample_gterm_ui(px: float, py: float, aa: float) -> Color:
    # Neutral Fluent desktop window base [12..204] x [20..192]
    x0, y0, x1, y1, r = 12.0, 20.0, 204.0, 192.0, 24.0
    col = sample_base_card(px, py, x0, y0, x1, y1, r, aa)
    if col[3] > 0.0:
        sd_card = sdf_round_rect(px, py, x0, y0, x1, y1, r)
        card_mask = cov(sd_card, aa)
        # Titlebar strip
        if py <= 60.0:
            t_strip = cov(py - 58.0, aa)
            col = over(col, (BASE_HEADER[0], BASE_HEADER[1], BASE_HEADER[2], BASE_HEADER[3] * t_strip * card_mask))
        c_sep = cov(sdf_segment(px, py, x0 + 4.0, 58.0, x1 - 4.0, 58.0, 2.0), aa) * card_mask
        if c_sep > 0.0:
            col = over(col, (BASE_INK_MUTED[0], BASE_INK_MUTED[1], BASE_INK_MUTED[2], 0.55 * c_sep))
        # Caption dots in titlebar
        for cx in (34.0, 50.0, 66.0):
            c_dot = cov(sdf_circle(px, py, cx, 39.0, 4.5), aa)
            if c_dot > 0.0:
                col = over(col, (BASE_INK[0], BASE_INK[1], BASE_INK[2], 0.65 * c_dot))
        # Left sidebar pane & right workspace pane
        c_side = cov(sdf_round_rect(px, py, 26.0, 72.0, 72.0, 174.0, 8.0), aa)
        if c_side > 0.0:
            col = over(col, (BASE_INK_MUTED[0], BASE_INK_MUTED[1], BASE_INK_MUTED[2], 0.30 * c_side))
        c_main = cov(sdf_round_rect(px, py, 82.0, 72.0, 190.0, 174.0, 8.0), aa)
        if c_main > 0.0:
            col = over(col, (BASE_PANE[0], BASE_PANE[1], BASE_PANE[2], 0.72 * c_main))
        # Subtle split bars in main pane
        c_line1 = cov(sdf_segment(px, py, 98.0, 96.0, 164.0, 96.0, 4.5), aa)
        c_line2 = cov(sdf_segment(px, py, 98.0, 120.0, 146.0, 120.0, 4.5), aa)
        c_lines = max(c_line1, c_line2)
        if c_lines > 0.0:
            col = over(col, (BASE_INK[0], BASE_INK[1], BASE_INK[2], 0.45 * c_lines))
    return apply_badge_overlay(col, px, py, aa)


def sample_gterm_session_worker(px: float, py: float, aa: float) -> Color:
    # Neutral generic console tile [14..202] x [18..194]
    x0, y0, x1, y1, r = 14.0, 18.0, 202.0, 194.0, 26.0
    col = sample_base_card(px, py, x0, y0, x1, y1, r, aa)
    if col[3] > 0.0:
        # Neutral monochrome >_ prompt
        sd_top = sdf_segment(px, py, 48.0, 66.0, 96.0, 102.0, 10.5)
        sd_bot = sdf_segment(px, py, 48.0, 138.0, 96.0, 102.0, 10.5)
        sd_bar = sdf_segment(px, py, 108.0, 134.0, 162.0, 134.0, 9.5)
        c_ink = cov(min(sd_top, sd_bot, sd_bar), aa)
        if c_ink > 0.0:
            col = over(col, (BASE_INK[0], BASE_INK[1], BASE_INK[2], 0.92 * c_ink))
    return apply_badge_overlay(col, px, py, aa)


def sample_gterm_mcp_daemon(px: float, py: float, aa: float) -> Color:
    # Neutral service / AI protocol node tile [14..202] x [14..202]
    x0, y0, x1, y1, r = 14.0, 14.0, 202.0, 202.0, 28.0
    col = sample_base_card(px, py, x0, y0, x1, y1, r, aa)
    if col[3] > 0.0:
        hub_x, hub_y = 104.0, 104.0
        satellites = ((56.0, 62.0), (156.0, 62.0), (56.0, 150.0))
        # Spokes
        sd_spokes = 1e9
        for sx, sy in satellites:
            sd_spokes = min(sd_spokes, sdf_segment(px, py, hub_x, hub_y, sx, sy, 6.0))
        c_spokes = cov(sd_spokes, aa)
        if c_spokes > 0.0:
            col = over(col, (BASE_INK_MUTED[0], BASE_INK_MUTED[1], BASE_INK_MUTED[2], 0.88 * c_spokes))
        # Outer ring around hub
        sd_ring = abs(sdf_circle(px, py, hub_x, hub_y, 26.0)) - 4.5
        c_ring = cov(sd_ring, aa)
        if c_ring > 0.0:
            col = over(col, (BASE_INK[0], BASE_INK[1], BASE_INK[2], 0.55 * c_ring))
        # Satellite nodes
        for sx, sy in satellites:
            c_sat = cov(sdf_circle(px, py, sx, sy, 13.5), aa)
            if c_sat > 0.0:
                col = over(col, (BASE_INK[0], BASE_INK[1], BASE_INK[2], 0.92 * c_sat))
        # Central hub
        c_hub = cov(sdf_circle(px, py, hub_x, hub_y, 18.0), aa)
        if c_hub > 0.0:
            col = over(col, (BASE_INK[0], BASE_INK[1], BASE_INK[2], c_hub))
    return apply_badge_overlay(col, px, py, aa)


Sampler = Callable[[float, float, float], Color]

ICON_SAMPLERS: dict[str, Sampler] = {
    "gterm": sample_gterm,
    "gterm-ui": sample_gterm_ui,
    "gterm-session-worker": sample_gterm_session_worker,
    "gterm-mcp-daemon": sample_gterm_mcp_daemon,
}


def render_rgba(sampler: Sampler, size: int) -> bytes:
    """Supersampled rasterization into tightly packed RGBA8 bytes."""
    ss = 4 if size <= 64 else 2
    inv_ss = 1.0 / ss
    inv_n = 1.0 / (ss * ss)
    scale = 256.0 / size
    aa = max(scale * 0.85, 0.45)

    out = bytearray(size * size * 4)
    idx = 0
    for py_i in range(size):
        for px_i in range(size):
            ar = ag = ab = aa_sum = 0.0
            for sy in range(ss):
                cy = (py_i + (sy + 0.5) * inv_ss) * scale
                for sx in range(ss):
                    cx = (px_i + (sx + 0.5) * inv_ss) * scale
                    r, g, b, a = sampler(cx, cy, aa)
                    ar += r * a
                    ag += g * a
                    ab += b * a
                    aa_sum += a
            a_avg = aa_sum * inv_n
            if a_avg > 1e-6:
                inv_a = 1.0 / aa_sum
                r8 = int(clamp(ar * inv_a) * 255.0 + 0.5)
                g8 = int(clamp(ag * inv_a) * 255.0 + 0.5)
                b8 = int(clamp(ab * inv_a) * 255.0 + 0.5)
                a8 = int(clamp(a_avg) * 255.0 + 0.5)
            else:
                r8 = g8 = b8 = a8 = 0
            out[idx] = r8
            out[idx + 1] = g8
            out[idx + 2] = b8
            out[idx + 3] = a8
            idx += 4
    return bytes(out)


def encode_png(rgba: bytes, width: int, height: int) -> bytes:
    def chunk(tag: bytes, data: bytes) -> bytes:
        crc = zlib.crc32(tag + data) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)

    stride = width * 4
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        start = y * stride
        raw.extend(rgba[start : start + stride])

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    idat = zlib.compress(bytes(raw), level=9)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def encode_ico(images: list[tuple[int, bytes]]) -> bytes:
    """Build a multi-resolution PNG-compressed Windows .ico container."""
    count = len(images)
    header = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    entries = bytearray()
    payload = bytearray()
    for size, png in images:
        dim = 0 if size >= 256 else size
        entries.extend(struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset))
        payload.extend(png)
        offset += len(png)
    return header + bytes(entries) + bytes(payload)


def encode_icns(chunks: list[tuple[bytes, bytes]]) -> bytes:
    body = bytearray()
    for ostype, png in chunks:
        assert len(ostype) == 4
        body.extend(ostype)
        body.extend(struct.pack(">I", 8 + len(png)))
        body.extend(png)
    total = 8 + len(body)
    return b"icns" + struct.pack(">I", total) + bytes(body)


# ---------------------------------------------------------------------------
# Clean Vector SVG Master Files
# ---------------------------------------------------------------------------

def svg_gterm_defs(prefix: str = "gt") -> str:
    return f"""    <linearGradient id="{prefix}-bg" x1="16" y1="16" x2="240" y2="240" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#1E293B"/>
      <stop offset="55%" stop-color="#0F172A"/>
      <stop offset="100%" stop-color="#090E1A"/>
    </linearGradient>
    <linearGradient id="{prefix}-rim" x1="16" y1="16" x2="16" y2="240" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#64748B" stop-opacity="0.55"/>
      <stop offset="100%" stop-color="#334155" stop-opacity="0.65"/>
    </linearGradient>
    <linearGradient id="{prefix}-chev" x1="60" y1="108" x2="114" y2="192" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#7EDEFF"/>
      <stop offset="100%" stop-color="#0078D4"/>
    </linearGradient>
    <linearGradient id="{prefix}-bar" x1="124" y1="182" x2="174" y2="182" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#F8FAFC"/>
      <stop offset="100%" stop-color="#BAE6FD"/>
    </linearGradient>"""


def svg_base_defs(prefix: str = "base") -> str:
    return f"""    <linearGradient id="{prefix}-card" x1="0" y1="16" x2="0" y2="200" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#F8FAFC"/>
      <stop offset="100%" stop-color="#CBD5E1"/>
    </linearGradient>"""


def write_svgs(svg_dir: Path) -> None:
    svg_dir.mkdir(parents=True, exist_ok=True)

    gterm_svg = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" fill="none">
  <defs>
{svg_gterm_defs("gt")}
    <radialGradient id="gt-glow" cx="50%" cy="50%" r="52%">
      <stop offset="0%" stop-color="#38BDF8" stop-opacity="0.16"/>
      <stop offset="100%" stop-color="#38BDF8" stop-opacity="0"/>
    </radialGradient>
  </defs>
  <rect x="16" y="16" width="224" height="224" rx="46" fill="url(#gt-bg)"/>
  <rect x="16" y="16" width="224" height="224" rx="46" fill="url(#gt-glow)"/>
  <line x1="30" y1="66" x2="226" y2="66" stroke="#94A3B8" stroke-opacity="0.22" stroke-width="3" stroke-linecap="round"/>
  <circle cx="50" cy="45" r="5.5" fill="#F87171" fill-opacity="0.90"/>
  <circle cx="70" cy="45" r="5.5" fill="#FBBF24" fill-opacity="0.90"/>
  <circle cx="90" cy="45" r="5.5" fill="#34D399" fill-opacity="0.90"/>
  <path d="M60 108 L114 150 L60 192" stroke="url(#gt-chev)" stroke-width="23" stroke-linecap="round" stroke-linejoin="round"/>
  <line x1="124" y1="182" x2="174" y2="182" stroke="url(#gt-bar)" stroke-width="20" stroke-linecap="round"/>
  <circle cx="190" cy="182" r="17.5" fill="#38BDF8"/>
  <circle cx="190" cy="182" r="7" fill="#E0F2FE"/>
  <rect x="17.6" y="17.6" width="220.8" height="220.8" rx="44.4" stroke="url(#gt-rim)" stroke-width="3.2"/>
</svg>
"""

    badge_group = """  <g transform="translate(122, 122) scale(0.5535714)">
    <rect x="0" y="0" width="224" height="224" rx="48" fill="url(#bd-bg)"/>
    <path d="M44 70 L102 114 L44 158" stroke="url(#bd-chev)" stroke-width="32" stroke-linecap="round" stroke-linejoin="round"/>
    <line x1="108" y1="154" x2="158" y2="154" stroke="url(#bd-bar)" stroke-width="26" stroke-linecap="round"/>
    <circle cx="174" cy="154" r="21" fill="#38BDF8"/>
    <circle cx="174" cy="154" r="8" fill="#E0F2FE"/>
    <rect x="2.75" y="2.75" width="218.5" height="218.5" rx="45.25" stroke="url(#bd-rim)" stroke-width="5.5"/>
  </g>"""

    badge_defs = """    <linearGradient id="bd-bg" x1="0" y1="0" x2="224" y2="224" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#1E293B"/>
      <stop offset="55%" stop-color="#0F172A"/>
      <stop offset="100%" stop-color="#090E1A"/>
    </linearGradient>
    <linearGradient id="bd-rim" x1="0" y1="0" x2="0" y2="224" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#64748B" stop-opacity="0.55"/>
      <stop offset="100%" stop-color="#334155" stop-opacity="0.65"/>
    </linearGradient>
    <linearGradient id="bd-chev" x1="44" y1="70" x2="102" y2="158" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#7EDEFF"/>
      <stop offset="100%" stop-color="#0078D4"/>
    </linearGradient>
    <linearGradient id="bd-bar" x1="108" y1="154" x2="158" y2="154" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#F8FAFC"/>
      <stop offset="100%" stop-color="#BAE6FD"/>
    </linearGradient>"""

    cutout_mask = """    <mask id="badge-cutout">
      <rect width="256" height="256" fill="#FFFFFF"/>
      <rect x="112.5" y="112.5" width="143" height="143" rx="36" fill="#000000"/>
    </mask>"""

    ui_svg = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" fill="none">
  <defs>
{svg_base_defs("ui")}
{badge_defs}
{cutout_mask}
  </defs>
  <g mask="url(#badge-cutout)">
    <rect x="12" y="20" width="192" height="172" rx="24" fill="url(#ui-card)" stroke="#64748B" stroke-opacity="0.85" stroke-width="5"/>
    <path d="M14.5 44 C14.5 31.8 24.3 22.5 36 22.5 H180 C191.7 22.5 201.5 31.8 201.5 44 V58 H14.5 Z" fill="#94A3B8" fill-opacity="0.45"/>
    <line x1="16" y1="58" x2="200" y2="58" stroke="#64748B" stroke-opacity="0.55" stroke-width="4"/>
    <circle cx="34" cy="39" r="4.5" fill="#334155" fill-opacity="0.65"/>
    <circle cx="50" cy="39" r="4.5" fill="#334155" fill-opacity="0.65"/>
    <circle cx="66" cy="39" r="4.5" fill="#334155" fill-opacity="0.65"/>
    <rect x="26" y="72" width="46" height="102" rx="8" fill="#64748B" fill-opacity="0.30"/>
    <rect x="82" y="72" width="108" height="102" rx="8" fill="#FFFFFF" fill-opacity="0.72"/>
    <line x1="98" y1="96" x2="164" y2="96" stroke="#334155" stroke-opacity="0.45" stroke-width="9" stroke-linecap="round"/>
    <line x1="98" y1="120" x2="146" y2="120" stroke="#334155" stroke-opacity="0.45" stroke-width="9" stroke-linecap="round"/>
  </g>
{badge_group}
</svg>
"""

    worker_svg = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" fill="none">
  <defs>
{svg_base_defs("wk")}
{badge_defs}
{cutout_mask}
  </defs>
  <g mask="url(#badge-cutout)">
    <rect x="14" y="18" width="188" height="176" rx="26" fill="url(#wk-card)" stroke="#64748B" stroke-opacity="0.85" stroke-width="5"/>
    <path d="M48 66 L96 102 L48 138" stroke="#334155" stroke-opacity="0.92" stroke-width="21" stroke-linecap="round" stroke-linejoin="round"/>
    <line x1="108" y1="134" x2="162" y2="134" stroke="#334155" stroke-opacity="0.92" stroke-width="19" stroke-linecap="round"/>
  </g>
{badge_group}
</svg>
"""

    mcp_svg = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" fill="none">
  <defs>
{svg_base_defs("mcp")}
{badge_defs}
{cutout_mask}
  </defs>
  <g mask="url(#badge-cutout)">
    <rect x="14" y="14" width="188" height="188" rx="28" fill="url(#mcp-card)" stroke="#64748B" stroke-opacity="0.85" stroke-width="5"/>
    <line x1="104" y1="104" x2="56" y2="62" stroke="#64748B" stroke-opacity="0.88" stroke-width="12" stroke-linecap="round"/>
    <line x1="104" y1="104" x2="156" y2="62" stroke="#64748B" stroke-opacity="0.88" stroke-width="12" stroke-linecap="round"/>
    <line x1="104" y1="104" x2="56" y2="150" stroke="#64748B" stroke-opacity="0.88" stroke-width="12" stroke-linecap="round"/>
    <circle cx="104" cy="104" r="26" stroke="#334155" stroke-opacity="0.55" stroke-width="9"/>
    <circle cx="56" cy="62" r="13.5" fill="#334155" fill-opacity="0.92"/>
    <circle cx="156" cy="62" r="13.5" fill="#334155" fill-opacity="0.92"/>
    <circle cx="56" cy="150" r="13.5" fill="#334155" fill-opacity="0.92"/>
    <circle cx="104" cy="104" r="18" fill="#334155"/>
  </g>
{badge_group}
</svg>
"""

    (svg_dir / "gterm.svg").write_text(gterm_svg, encoding="utf-8", newline="\n")
    (svg_dir / "gterm-ui.svg").write_text(ui_svg, encoding="utf-8", newline="\n")
    (svg_dir / "gterm-session-worker.svg").write_text(worker_svg, encoding="utf-8", newline="\n")
    (svg_dir / "gterm-mcp-daemon.svg").write_text(mcp_svg, encoding="utf-8", newline="\n")


def composite_preview(out_path: Path, png_cache: dict[tuple[str, int], bytes]) -> None:
    """Render a contact sheet comparing all 4 icons at 256, 64, 32, 16 on light & dark."""
    names = ["gterm", "gterm-ui", "gterm-session-worker", "gterm-mcp-daemon"]
    sizes = [128, 64, 32, 16]
    # Re-render 128 on demand for preview clarity
    width, height = 960, 420
    canvas = bytearray(width * height * 4)

    def fill_rect(x0: int, y0: int, w: int, h: int, r: int, g: int, b: int) -> None:
        for yy in range(max(0, y0), min(height, y0 + h)):
            row_off = yy * width * 4
            for xx in range(max(0, x0), min(width, x0 + w)):
                p = row_off + xx * 4
                canvas[p] = r
                canvas[p + 1] = g
                canvas[p + 2] = b
                canvas[p + 3] = 255

    # Top half: Light Explorer (#F3F3F3), Bottom half: Dark Task Manager (#202020)
    fill_rect(0, 0, width, height // 2, 243, 243, 243)
    fill_rect(0, height // 2, width, height - height // 2, 32, 32, 32)

    rendered_128 = {n: render_rgba(ICON_SAMPLERS[n], 128) for n in names}
    rendered_sizes = {
        (n, s): (rendered_128[n] if s == 128 else render_rgba(ICON_SAMPLERS[n], s))
        for n in names
        for s in sizes
    }

    def blit(rgba: bytes, sz: int, dx: int, dy: int) -> None:
        for yy in range(sz):
            cy = dy + yy
            if cy < 0 or cy >= height:
                continue
            for xx in range(sz):
                cx = dx + xx
                if cx < 0 or cx >= width:
                    continue
                si = (yy * sz + xx) * 4
                sa = rgba[si + 3] / 255.0
                if sa <= 0.0:
                    continue
                di = (cy * width + cx) * 4
                inv = 1.0 - sa
                canvas[di] = int(rgba[si] * sa + canvas[di] * inv + 0.5)
                canvas[di + 1] = int(rgba[si + 1] * sa + canvas[di + 1] * inv + 0.5)
                canvas[di + 2] = int(rgba[si + 2] * sa + canvas[di + 2] * inv + 0.5)
                canvas[di + 3] = 255

    col_w = width // len(names)
    for col, name in enumerate(names):
        base_x = col * col_w + 24
        # Light strip
        blit(rendered_sizes[(name, 128)], 128, base_x, 24)
        blit(rendered_sizes[(name, 64)], 64, base_x + 138, 24)
        blit(rendered_sizes[(name, 32)], 32, base_x + 138, 98)
        blit(rendered_sizes[(name, 16)], 16, base_x + 178, 106)
        # Dark strip
        blit(rendered_sizes[(name, 128)], 128, base_x, height // 2 + 24)
        blit(rendered_sizes[(name, 64)], 64, base_x + 138, height // 2 + 24)
        blit(rendered_sizes[(name, 32)], 32, base_x + 138, height // 2 + 98)
        blit(rendered_sizes[(name, 16)], 16, base_x + 178, height // 2 + 106)

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_bytes(encode_png(bytes(canvas), width, height))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--preview",
        type=Path,
        default=None,
        help="Optional path to write a light/dark contact sheet PNG",
    )
    args = parser.parse_args()

    icons_dir = ROOT / "assets" / "icons"
    svg_dir = icons_dir / "svg"
    linux_dir = icons_dir / "linux"
    brand_dir = ROOT / "ui" / "icons" / "brand"

    write_svgs(svg_dir)

    png_cache: dict[tuple[str, int], bytes] = {}

    def get_png(name: str, size: int) -> bytes:
        key = (name, size)
        if key not in png_cache:
            rgba = render_rgba(ICON_SAMPLERS[name], size)
            png_cache[key] = encode_png(rgba, size, size)
        return png_cache[key]

    # 1. Windows .ico for all 4 binaries
    ico_sizes = [16, 24, 32, 48, 64, 256]
    icons_dir.mkdir(parents=True, exist_ok=True)
    for name in ICON_SAMPLERS:
        entries = [(sz, get_png(name, sz)) for sz in ico_sizes]
        (icons_dir / f"{name}.ico").write_bytes(encode_ico(entries))

    # 2. macOS .icns for the app bundle
    icns_specs = [
        (b"icp4", 16),
        (b"icp5", 32),
        (b"icp6", 64),
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic09", 512),
        (b"ic10", 1024),
        (b"ic11", 32),
        (b"ic12", 64),
        (b"ic13", 256),
        (b"ic14", 512),
    ]
    icns_chunks = [(ostype, get_png("gterm", sz)) for ostype, sz in icns_specs]
    (icons_dir / "gterm.icns").write_bytes(encode_icns(icns_chunks))

    # 3. Linux hicolor PNGs
    for sz in (16, 32, 48, 128, 256, 512):
        folder = linux_dir / f"{sz}x{sz}"
        folder.mkdir(parents=True, exist_ok=True)
        (folder / "gterm.png").write_bytes(get_png("gterm", sz))

    # 4. Slint UI brand PNGs
    brand_dir.mkdir(parents=True, exist_ok=True)
    for sz in (32, 64, 96, 256):
        (brand_dir / f"gterm-{sz}.png").write_bytes(get_png("gterm", sz))

    if args.preview is not None:
        composite_preview(args.preview, png_cache)
        print(f"wrote preview contact sheet: {args.preview}")

    print("generated gTerm SVG, .ico (x4), .icns, Linux hicolor PNGs, and ui/icons/brand/ PNGs")


if __name__ == "__main__":
    main()
