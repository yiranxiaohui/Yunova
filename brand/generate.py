#!/usr/bin/env python3
"""Generate every Yunova brand icon from one parametric definition.

    cd brand && python3 generate.py

The mark is a three-ray nova whose long lower ray also reads as the stem of a
"Y": one shape carries both the product name and the "nova" in Yunova. It is
defined here as geometry rather than shipped as a hand-drawn file so the
favicon, app tile and store logos can never drift apart, and so proportions
stay tunable from a single place.

Outputs (all overwritten):
  web/public/        logo.svg/.png, favicon.svg/.png/.ico, apple-touch-icon.*,
                     logo-mark.svg
  desktop/icons/     Tauri bundle set incl. icon.ico and icon.icns

Requires Pillow (raster packing) and, for SVG rasterisation, `bun install` in
this directory to provide @resvg/resvg-js. Without bun the SVG masters are
still regenerated and the existing rasters are left untouched.
"""

from __future__ import annotations

import math
import pathlib
import shutil
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent
WEB = REPO / "web/public"
ICONS = REPO / "desktop/icons"
WORK = HERE / "build"
PNG = WORK / "png"

# Brand gradient: light violet shoulder into the deep indigo used by the app's
# --primary ramp, so the icon sits in the same palette as the UI.
TILE_TOP, TILE_MID, TILE_BOT = "#9A6BFF", "#6D3BD1", "#2C0B66"
BRAND_SOLID = "#6D3BD1"


# ---------------------------------------------------------------------------
# geometry
# ---------------------------------------------------------------------------


def _pt(cx: float, cy: float, deg: float, r: float) -> tuple[float, float]:
    a = math.radians(deg)
    return (cx + math.sin(a) * r, cy - math.cos(a) * r)


def _star(cx: float, cy: float, nodes, bow: float = 0.18) -> str:
    """Closed path through alternating tip/valley polar nodes.

    Every edge is a quadratic whose control point is pulled `bow` of the way
    toward the center, which is what turns a plain polygon star into rays that
    taper like light. Tips and valleys stay exactly on the curve, so the
    silhouette is predictable at 16px.
    """
    p = [_pt(cx, cy, d, r) for d, r in nodes]
    out = [f"M{p[0][0]:.2f} {p[0][1]:.2f}"]
    for i, a in enumerate(p):
        b = p[(i + 1) % len(p)]
        mx, my = (a[0] + b[0]) / 2, (a[1] + b[1]) / 2
        out.append(
            f"Q{mx + (cx - mx) * bow:.2f} {my + (cy - my) * bow:.2f} {b[0]:.2f} {b[1]:.2f}"
        )
    return " ".join(out) + " Z"


def nova_y(cx: float, cy: float, s: float = 1.0, arm_deg: float = 42) -> str:
    """The mark: two raised arms plus an elongated lower ray.

    `arm_deg` 42 is the compromise that keeps the Y readable while the arms
    still look like they are thrown outward by the burst; flatter arms read as
    a checkmark, steeper ones as a dart.
    """
    arm, stem, top_v, side_v = 172 * s, 164 * s, 64 * s, 38 * s
    nodes = [
        (-arm_deg, arm),
        (0, top_v),
        (arm_deg, arm),
        ((arm_deg + 180) / 2, side_v),
        (180, stem),
        (-(arm_deg + 180) / 2, side_v),
    ]
    return _star(cx, cy, nodes)


def spark(cx: float, cy: float, r: float) -> str:
    """Four-point companion sparkle: the "AI" cue, kept subordinate to the mark."""
    nodes = []
    for i in range(4):
        nodes += [(i * 90, r), (i * 90 + 45, r * 0.3)]
    return _star(cx, cy, nodes, bow=0.5)


# ---------------------------------------------------------------------------
# svg masters
# ---------------------------------------------------------------------------


def _defs(idp: str) -> str:
    # Gradient ids are prefixed per file because several of these SVGs can be
    # inlined into one document, where duplicate ids would silently cross-wire.
    return f"""  <defs>
    <linearGradient id="{idp}-tile" x1="0.08" y1="0" x2="0.92" y2="1">
      <stop offset="0" stop-color="{TILE_TOP}"/>
      <stop offset="0.45" stop-color="{TILE_MID}"/>
      <stop offset="1" stop-color="{TILE_BOT}"/>
    </linearGradient>
    <radialGradient id="{idp}-halo" cx="0.5" cy="0.4" r="0.62">
      <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.28"/>
      <stop offset="1" stop-color="#FFFFFF" stop-opacity="0"/>
    </radialGradient>
    <radialGradient id="{idp}-core" cx="0.5" cy="0.5" r="0.5">
      <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.9"/>
      <stop offset="0.5" stop-color="#FFFFFF" stop-opacity="0.3"/>
      <stop offset="1" stop-color="#FFFFFF" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="{idp}-mark" x1="0.5" y1="0.06" x2="0.5" y2="1">
      <stop offset="0" stop-color="#FFFFFF"/>
      <stop offset="0.55" stop-color="#FCFAFF"/>
      <stop offset="1" stop-color="#DCCBFF"/>
    </linearGradient>
  </defs>
"""


def tile_icon(scale: float = 1.0, radius: int = 112, idp: str = "y") -> str:
    """Rounded-tile icon used for the site logo and desktop bundles."""
    cx, cy = 256, 246
    sx = 400 * scale + 256 * (1 - scale)
    sy = 124 * scale + 246 * (1 - scale)
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" role="img" aria-label="Yunova">
{_defs(idp)}  <rect width="512" height="512" rx="{radius}" fill="url(#{idp}-tile)"/>
  <rect width="512" height="512" rx="{radius}" fill="url(#{idp}-halo)"/>
  <circle cx="{cx}" cy="{cy}" r="{132 * scale:.0f}" fill="url(#{idp}-core)"/>
  <path d="{nova_y(cx, cy, scale)}" fill="url(#{idp}-mark)"/>
  <path d="{spark(sx, sy, 30 * scale)}" fill="#FFFFFF" fill-opacity="0.9"/>
  <rect x="1.5" y="1.5" width="509" height="509" rx="{radius - 1.5}" fill="none" stroke="#FFFFFF" stroke-opacity="0.18" stroke-width="3"/>
</svg>
"""


def glyph_only(color: str = BRAND_SOLID) -> str:
    """Tile-less single-color mark, for print, watermarks and recoloring."""
    cx, cy = 256, 262
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" role="img" aria-label="Yunova">
  <path d="{nova_y(cx, cy, 1.12)}" fill="{color}"/>
  <path d="{spark(452, 104, 34)}" fill="{color}" fill-opacity="0.85"/>
</svg>
"""


def ios_icon(idp: str = "i") -> str:
    """Home-screen icon: square and opaque because the OS applies its own mask.

    A pre-rounded, transparent icon here would be composited onto black and
    show dark corners inside that mask.
    """
    cx, cy = 256, 250
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" role="img" aria-label="Yunova">
{_defs(idp)}  <rect width="512" height="512" fill="url(#{idp}-tile)"/>
  <rect width="512" height="512" fill="url(#{idp}-halo)"/>
  <circle cx="{cx}" cy="{cy}" r="128" fill="url(#{idp}-core)"/>
  <path d="{nova_y(cx, cy, 0.95)}" fill="url(#{idp}-mark)"/>
  <path d="{spark(392, 130, 28)}" fill="#FFFFFF" fill-opacity="0.9"/>
</svg>
"""


MASTERS = {
    "logo.svg": tile_icon(idp="y"),
    # Below ~32px the tile's outer ring and the spark's fine points turn to
    # mush, so the favicon cut trades padding for a larger glyph.
    "favicon.svg": tile_icon(scale=1.16, radius=96, idp="f"),
    "apple-touch-icon.svg": ios_icon(),
    "logo-mark.svg": glyph_only(),
}

# Tauri's bundle set plus the Windows Store logo sizes.
TILE_SIZES = [30, 32, 44, 50, 64, 71, 89, 107, 128, 142, 150, 256, 284, 310, 512, 1024]


def write_masters() -> None:
    WORK.mkdir(parents=True, exist_ok=True)
    for name, svg in MASTERS.items():
        (WORK / name).write_text(svg)
        # The SVG masters double as shipped assets, so the site serves exactly
        # what this script produced.
        (WEB / name).write_text(svg)
    print(f"svg masters -> {WORK} and {WEB}")


def rasterise() -> bool:
    if shutil.which("bun") is None:
        print("bun not found: skipped raster regeneration", file=sys.stderr)
        return False
    if not (HERE / "node_modules/@resvg/resvg-js").exists():
        print("installing @resvg/resvg-js ...")
        if subprocess.run(["bun", "install"], cwd=HERE).returncode != 0:
            print("bun install failed: skipped rasters", file=sys.stderr)
            return False
    return subprocess.run(["bun", "run", "render.mjs"], cwd=HERE).returncode == 0


def pack() -> None:
    from PIL import Image

    def load(name: str) -> "Image.Image":
        return Image.open(PNG / name).convert("RGBA")

    load("logo-512.png").save(WEB / "logo.png", optimize=True)
    load("favicon-64.png").save(WEB / "favicon.png", optimize=True)
    # Flattened to RGB on purpose; see ios_icon().
    load("ios-180.png").convert("RGB").save(WEB / "apple-touch-icon.png", optimize=True)
    # 16-48px only: bigger rasters belong in favicon.svg/.png, and packing 128
    # and 256 in here inflated the .ico tenfold for no visible gain.
    load("favicon-48.png").save(WEB / "favicon.ico", sizes=[(16, 16), (32, 32), (48, 48)])

    for size in (32, 64, 128):
        load(f"tile-{size}.png").save(ICONS / f"{size}x{size}.png", optimize=True)
    load("tile-256.png").save(ICONS / "128x128@2x.png", optimize=True)
    load("tile-512.png").save(ICONS / "icon.png", optimize=True)
    for size, name in [
        (30, "Square30x30Logo"),
        (44, "Square44x44Logo"),
        (50, "StoreLogo"),
        (71, "Square71x71Logo"),
        (89, "Square89x89Logo"),
        (107, "Square107x107Logo"),
        (142, "Square142x142Logo"),
        (150, "Square150x150Logo"),
        (284, "Square284x284Logo"),
        (310, "Square310x310Logo"),
    ]:
        load(f"tile-{size}.png").save(ICONS / f"{name}.png", optimize=True)
    load("tile-256.png").save(
        ICONS / "icon.ico",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    # Pillow writes the full multi-resolution ICNS table from one 1024px source.
    load("tile-1024.png").save(ICONS / "icon.icns")
    print(f"rasters -> {WEB} and {ICONS}")


def main() -> int:
    write_masters()
    if not rasterise():
        return 1
    pack()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
