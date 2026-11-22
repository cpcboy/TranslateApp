#!/usr/bin/env python3
"""
生成 SENSE 全套应用图标资源（方案 A：折光之 S · Obsidian Prism）。

真相源说明：
- 本脚本（sense/tools/generate-icons.py）是图标几何与配色的唯一源码真相；
- sense/src-tauri/icons/icon.svg 为本脚本导出的矢量副本，勿直接手改（重新运行本脚本会重新生成全部产物）。

设计规范：
- 严格遵循 macOS 连续曲率圆角规范（1024×1024 画布内 824×824 连续曲率圆角方砖，r=185.4, smoothing=0.6）
- 配色与应用 UI token 统一：深曜石底（#1B1B24 → #07070B）、珍珠白上弧（#FFFFFF → #CFCBC3）、朱砂橙下弧（#FF9C7B → #DF3110）
- 尺寸 ≤ 32px（16/24/30/32）使用专属光学微调 SVG 并由 Inkscape 按目标尺寸原生矢量导出，避免大倍率 Lanczos 降采样负旁瓣残值，确保 1x 屏幕与 Finder 列表锐利清晰

产物（全部先在临时目录完整生成并校验后，再原子替换至 sense/src-tauri/icons/）：
- icon.svg               生成的 1024×1024 矢量副本
- icon.icns              macOS 全 10 档分辨率图标（16..512@2x，由系统 iconutil 封装）
- icon.ico               Windows 多分辨率 RGBA ICO（16/24/32/48/64/128/256）
- icon.png               512×512 RGBA PNG
- 32x32.png / 128x128.png / 128x128@2x.png 及 Windows Store 全套 Square*Logo.png / StoreLogo.png（全 RGBA）

依赖：inkscape、Pillow (PIL)、macOS /usr/bin/iconutil。
用法：python3 sense/tools/generate-icons.py
"""

from __future__ import annotations

import math
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

try:
    from PIL import Image
except ModuleNotFoundError as exc:
    raise SystemExit("缺少 Python 依赖 Pillow（PIL），请先安装：pip3 install pillow") from exc

SENSE_ROOT = Path(__file__).resolve().parents[1]
ICONS_DIR = SENSE_ROOT / "src-tauri" / "icons"


def apple_squircle_path(x0: float = 100.0, y0: float = 100.0, size: float = 824.0, r: float = 185.4, smoothing: float = 0.6) -> str:
    """Exact Apple macOS continuous-corner squircle SVG path (Big Sur – Tahoe HIG)."""
    p = min(size / 2.0, (1.0 + smoothing) * r)
    theta = math.radians(45.0 * smoothing)
    arc_angle = math.radians(90.0 * (1.0 - smoothing))
    arc_section = math.sin(arc_angle / 2.0) * r * math.sqrt(2.0)
    c = r * math.tan(theta / 2.0) * math.cos(theta)
    d = r * math.tan(theta / 2.0) * math.sin(theta)
    b = (p - arc_section - c - d) / 3.0
    a = 2.0 * b

    L, T, R, B = x0, y0, x0 + size, y0 + size
    parts = [
        f"M {L + p:.2f} {T:.2f}",
        f"L {R - p:.2f} {T:.2f}",
        f"C {R - (p - a):.2f} {T:.2f}, {R - (p - a - b):.2f} {T:.2f}, {R - (p - a - b - c):.2f} {T + d:.2f}",
        f"A {r:.2f} {r:.2f} 0 0 1 {R - d:.2f} {T + (p - a - b - c):.2f}",
        f"C {R:.2f} {T + (p - a - b):.2f}, {R:.2f} {T + (p - a):.2f}, {R:.2f} {T + p:.2f}",
        f"L {R:.2f} {B - p:.2f}",
        f"C {R:.2f} {B - (p - a):.2f}, {R:.2f} {B - (p - a - b):.2f}, {R - d:.2f} {B - (p - a - b - c):.2f}",
        f"A {r:.2f} {r:.2f} 0 0 1 {R - (p - a - b - c):.2f} {B - d:.2f}",
        f"C {R - (p - a - b):.2f} {B:.2f}, {R - (p - a):.2f} {B:.2f}, {R - p:.2f} {B:.2f}",
        f"L {L + p:.2f} {B:.2f}",
        f"C {L + (p - a):.2f} {B:.2f}, {L + (p - a - b):.2f} {B:.2f}, {L + (p - a - b - c):.2f} {B - d:.2f}",
        f"A {r:.2f} {r:.2f} 0 0 1 {L + d:.2f} {B - (p - a - b - c):.2f}",
        f"C {L:.2f} {B - (p - a - b):.2f}, {L:.2f} {B - (p - a):.2f}, {L:.2f} {B - p:.2f}",
        f"L {L:.2f} {T + p:.2f}",
        f"C {L:.2f} {T + (p - a):.2f}, {L:.2f} {T + (p - a - b):.2f}, {L + d:.2f} {T + (p - a - b - c):.2f}",
        f"A {r:.2f} {r:.2f} 0 0 1 {L + (p - a - b - c):.2f} {T + d:.2f}",
        f"C {L + (p - a - b):.2f} {T:.2f}, {L + (p - a):.2f} {T:.2f}, {L + p:.2f} {T:.2f}",
        "Z",
    ]
    return " ".join(parts)


def svg_drop_shadow_filter(fid: str, dx: float, dy: float, blur: float, opacity: float, color: str = "#000000") -> str:
    return f'''<filter id="{fid}" x="-35%" y="-35%" width="170%" height="175%">
      <feGaussianBlur in="SourceAlpha" stdDeviation="{blur}" result="blur"/>
      <feOffset in="blur" dx="{dx}" dy="{dy}" result="off"/>
      <feFlood flood-color="{color}" flood-opacity="{opacity}" result="flood"/>
      <feComposite in="flood" in2="off" operator="in" result="shadow"/>
      <feMerge>
        <feMergeNode in="shadow"/>
        <feMergeNode in="SourceGraphic"/>
      </feMerge>
    </filter>'''


def arc_ribbon_path(cx: float, cy: float, r_in: float, r_out: float, a_start_deg: float, a_end_deg: float) -> str:
    a1 = math.radians(a_start_deg)
    a2 = math.radians(a_end_deg)
    ccw = a_end_deg < a_start_deg
    sweep_outer = 0 if ccw else 1
    sweep_inner = 1 if ccw else 0
    delta = abs(a_end_deg - a_start_deg)
    large_arc = 1 if delta > 180 else 0

    ox1, oy1 = cx + r_out * math.cos(a1), cy + r_out * math.sin(a1)
    ox2, oy2 = cx + r_out * math.cos(a2), cy + r_out * math.sin(a2)
    ix2, iy2 = cx + r_in * math.cos(a2), cy + r_in * math.sin(a2)
    ix1, iy1 = cx + r_in * math.cos(a1), cy + r_in * math.sin(a1)
    cap_r = (r_out - r_in) / 2.0
    cap_sweep = 0 if ccw else 1
    return (
        f"M {ox1:.2f} {oy1:.2f} "
        f"A {r_out:.2f} {r_out:.2f} 0 {large_arc} {sweep_outer} {ox2:.2f} {oy2:.2f} "
        f"A {cap_r:.2f} {cap_r:.2f} 0 0 {cap_sweep} {ix2:.2f} {iy2:.2f} "
        f"A {r_in:.2f} {r_in:.2f} 0 {large_arc} {sweep_inner} {ix1:.2f} {iy1:.2f} "
        f"A {cap_r:.2f} {cap_r:.2f} 0 0 {cap_sweep} {ox1:.2f} {oy1:.2f} Z"
    )


def arc_spine_path(cx: float, cy: float, r_mid: float, a_start_deg: float, a_end_deg: float) -> str:
    a1 = math.radians(a_start_deg)
    a2 = math.radians(a_end_deg)
    ccw = a_end_deg < a_start_deg
    sweep = 0 if ccw else 1
    large_arc = 1 if abs(a_end_deg - a_start_deg) > 180 else 0
    x1, y1 = cx + r_mid * math.cos(a1), cy + r_mid * math.sin(a1)
    x2, y2 = cx + r_mid * math.cos(a2), cy + r_mid * math.sin(a2)
    return f"M {x1:.2f} {y1:.2f} A {r_mid:.2f} {r_mid:.2f} 0 {large_arc} {sweep} {x2:.2f} {y2:.2f}"


def build_sense_icon_svg(small_optical: bool = False) -> str:
    """
    Build SENSE Obsidian Prism S SVG (1024×1024).
    When small_optical=True (for <= 32px targets), widens the prism slit slightly
    and boosts stroke weight + rim clarity so tiny Finder/Spotlight renders stay crisp.
    """
    squircle = apple_squircle_path(100.0, 100.0, 824.0, r=185.4, smoothing=0.6)
    squircle_inner = apple_squircle_path(101.75, 101.75, 820.5, r=183.65, smoothing=0.6)

    if small_optical:
        r_in, r_out = 72.0, 164.0  # 92px bold stroke for <= 32px crispness
        slit_half = 21.0           # 42px slit -> clear separation at 32px & crisp transition at 16px
        rim_width = 7.0
        rim_top_op = 0.42
        shadow_op = 0.28
    else:
        r_in, r_out = 77.0, 159.0  # 82px sculpted stroke
        slit_half = 10.0           # 20px razor-sharp diagonal prism cut
        rim_width = 3.5
        rim_top_op = 0.32
        shadow_op = 0.44

    r_mid = (r_in + r_out) / 2.0
    cy_top = 512.0 - r_mid
    cy_bot = 512.0 + r_mid

    upper_arc = arc_ribbon_path(512.0, cy_top, r_in, r_out, -38.0, -298.0)
    lower_arc = arc_ribbon_path(512.0, cy_bot, r_in, r_out, 142.0, -118.0)
    upper_spine = arc_spine_path(512.0, cy_top, r_mid, -38.0, -270.0)
    lower_spine = arc_spine_path(512.0, cy_bot, r_mid, 142.0, -90.0)

    mask_w = 220.0
    upper_cut_x = -slit_half
    lower_cut_x = slit_half - mask_w

    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
  <defs>
    {svg_drop_shadow_filter("tileShadow", 0, 14, 16, shadow_op)}
    {svg_drop_shadow_filter("glyphShadow", 0, 16, 16, 0.64)}

    <clipPath id="squircleClip"><path d="{squircle}"/></clipPath>
    <clipPath id="upperArcClip"><path d="{upper_arc}"/></clipPath>
    <clipPath id="lowerArcClip"><path d="{lower_arc}"/></clipPath>

    <mask id="upperHalfMask">
      <rect x="0" y="0" width="1024" height="1024" fill="white"/>
      <g transform="translate(512, 512) rotate(24)">
        <rect x="{upper_cut_x:.2f}" y="-96" width="{mask_w:.2f}" height="192" fill="black"/>
      </g>
    </mask>
    <mask id="lowerHalfMask">
      <rect x="0" y="0" width="1024" height="1024" fill="white"/>
      <g transform="translate(512, 512) rotate(24)">
        <rect x="{lower_cut_x:.2f}" y="-96" width="{mask_w:.2f}" height="192" fill="black"/>
      </g>
    </mask>

    <linearGradient id="bgGrad" x1="0" y1="100" x2="0" y2="924" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#1B1B24"/>
      <stop offset="46%" stop-color="#101016"/>
      <stop offset="100%" stop-color="#07070B"/>
    </linearGradient>

    <radialGradient id="topSheen" cx="512" cy="115" r="540" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#FFFFFF" stop-opacity="0.10"/>
      <stop offset="55%" stop-color="#FFFFFF" stop-opacity="0.02"/>
      <stop offset="100%" stop-color="#FFFFFF" stop-opacity="0"/>
    </radialGradient>

    <radialGradient id="coralAmbient" cx="560" cy="645" r="355" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#FF5530" stop-opacity="0.38"/>
      <stop offset="48%" stop-color="#FF4B26" stop-opacity="0.12"/>
      <stop offset="100%" stop-color="#FF4B26" stop-opacity="0"/>
    </radialGradient>

    <radialGradient id="pearlAmbient" cx="460" cy="375" r="310" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#EEF2FF" stop-opacity="0.17"/>
      <stop offset="55%" stop-color="#C8D6F5" stop-opacity="0.04"/>
      <stop offset="100%" stop-color="#C8D6F5" stop-opacity="0"/>
    </radialGradient>

    <linearGradient id="rimGrad" x1="0" y1="100" x2="0" y2="924" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#FFFFFF" stop-opacity="{rim_top_op:.2f}"/>
      <stop offset="22%" stop-color="#FFFFFF" stop-opacity="0.10"/>
      <stop offset="78%" stop-color="#FFFFFF" stop-opacity="0.04"/>
      <stop offset="100%" stop-color="#FF8C6B" stop-opacity="0.18"/>
    </linearGradient>

    <linearGradient id="upperBladeGrad" x1="360" y1="235" x2="635" y2="545" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#FFFFFF"/>
      <stop offset="52%" stop-color="#F5F3EE"/>
      <stop offset="100%" stop-color="#CFCBC3"/>
    </linearGradient>

    <linearGradient id="lowerBladeGrad" x1="425" y1="485" x2="655" y2="790" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#FF9C7B"/>
      <stop offset="40%" stop-color="#FF613E"/>
      <stop offset="100%" stop-color="#DF3110"/>
    </linearGradient>

    <!-- Wide filter regions prevent objectBoundingBox clipping in strict SVG 1.1 renderers (CoreSVG / WebKit) -->
    <filter id="coralBloom" x="-80%" y="-80%" width="260%" height="260%">
      <feGaussianBlur stdDeviation="26"/>
    </filter>
    <filter id="pearlBloom" x="-80%" y="-80%" width="260%" height="260%">
      <feGaussianBlur stdDeviation="18"/>
    </filter>
    <filter id="softSpec" x="-50%" y="-50%" width="200%" height="200%">
      <feGaussianBlur stdDeviation="11"/>
    </filter>
  </defs>

  <path d="{squircle}" fill="#09090D" filter="url(#tileShadow)"/>

  <g clip-path="url(#squircleClip)">
    <rect x="100" y="100" width="824" height="824" fill="url(#bgGrad)"/>
    <rect x="100" y="100" width="824" height="824" fill="url(#topSheen)"/>
    <rect x="100" y="100" width="824" height="824" fill="url(#pearlAmbient)"/>
    <rect x="100" y="100" width="824" height="824" fill="url(#coralAmbient)"/>

    <circle cx="512" cy="512" r="326" fill="none" stroke="#FFFFFF" stroke-opacity="0.035" stroke-width="2"/>

    <g transform="translate(512, 512) scale(1.09, 1.0) translate(-512, -512)">
      <path d="{upper_spine}" fill="none" stroke="#FFFFFF" stroke-opacity="0.18" stroke-width="76" stroke-linecap="round" filter="url(#pearlBloom)"/>
      <path d="{lower_spine}" fill="none" stroke="#FF5230" stroke-opacity="0.48" stroke-width="82" stroke-linecap="round" filter="url(#coralBloom)"/>

      <g filter="url(#glyphShadow)">
        <g mask="url(#upperHalfMask)">
          <path d="{upper_arc}" fill="url(#upperBladeGrad)"/>
          <g clip-path="url(#upperArcClip)">
            <path d="{upper_spine}" fill="none" stroke="#FFFFFF" stroke-opacity="0.62" stroke-width="26" stroke-linecap="round" transform="translate(0, -18)" filter="url(#softSpec)"/>
            <path d="{upper_arc}" fill="none" stroke="#FFFFFF" stroke-opacity="0.72" stroke-width="4" transform="translate(0, 2)"/>
          </g>
        </g>

        <g mask="url(#lowerHalfMask)">
          <path d="{lower_arc}" fill="url(#lowerBladeGrad)"/>
          <g clip-path="url(#lowerArcClip)">
            <path d="{lower_spine}" fill="none" stroke="#FFD6C2" stroke-opacity="0.58" stroke-width="26" stroke-linecap="round" transform="translate(0, -16)" filter="url(#softSpec)"/>
            <path d="{lower_arc}" fill="none" stroke="#FFD8C8" stroke-opacity="0.60" stroke-width="4" transform="translate(0, 2)"/>
          </g>
        </g>
      </g>
    </g>

    <path d="{squircle_inner}" fill="none" stroke="url(#rimGrad)" stroke-width="{rim_width:.1f}"/>
  </g>
</svg>
'''


def run_cmd(cmd: list[str], step: str) -> None:
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout or "").strip()
        msg = f"{step} 失败（exit {proc.returncode}）"
        if detail:
            msg = f"{msg}：\n{detail}"
        raise SystemExit(msg)


def render_svg_png(svg_path: Path, out_png: Path, render_size: int) -> Image.Image:
    run_cmd(
        [
            "inkscape",
            str(svg_path),
            "--export-type=png",
            f"--export-filename={out_png}",
            f"--export-width={render_size}",
            f"--export-height={render_size}",
        ],
        step=f"Inkscape 渲染 {svg_path.name} ({render_size}x{render_size})",
    )
    with Image.open(out_png) as img:
        return img.convert("RGBA")


def save_rgba_png(image: Image.Image, dest: Path) -> None:
    image.convert("RGBA").save(dest, format="PNG", optimize=True)


def main() -> None:
    for tool in ("inkscape", "iconutil"):
        if shutil.which(tool) is None:
            raise SystemExit(f"缺少依赖工具：{tool}")

    ICONS_DIR.mkdir(parents=True, exist_ok=True)

    # 先在临时目录完整生成全部产物，全部成功后再一次性替换至 ICONS_DIR（避免失败留下半更新状态）
    with tempfile.TemporaryDirectory(prefix="sense-icons-") as tmp:
        tmp_dir = Path(tmp)
        staged_dir = tmp_dir / "staged"
        staged_dir.mkdir()

        # 1. 生成主矢量副本 icon.svg 与小尺寸光学微调 SVG
        staged_svg = staged_dir / "icon.svg"
        staged_svg.write_text(build_sense_icon_svg(small_optical=False), encoding="utf-8")

        small_svg = tmp_dir / "icon-small.svg"
        small_svg.write_text(build_sense_icon_svg(small_optical=True), encoding="utf-8")

        # 2. 超采样渲染大尺寸母版（2048×2048）；<=32px 直接由 Inkscape 按目标尺寸原生矢量导出（零 Lanczos 负旁瓣）
        master_large = render_svg_png(staged_svg, tmp_dir / "master-2048.png", 2048)
        small_cache: dict[int, Image.Image] = {}

        def image_for_size(size: int) -> Image.Image:
            if size <= 32:
                if size not in small_cache:
                    small_cache[size] = render_svg_png(small_svg, tmp_dir / f"small-{size}.png", size)
                return small_cache[size]
            return master_large.resize((size, size), Image.Resampling.LANCZOS).convert("RGBA")

        # 3. 生成 macOS 全 10 档 iconset 并由 iconutil 打包 icon.icns
        iconset_dir = tmp_dir / "SENSE.iconset"
        iconset_dir.mkdir()
        icns_specs = [
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
        for fname, sz in icns_specs:
            save_rgba_png(image_for_size(sz), iconset_dir / fname)

        run_cmd(
            ["iconutil", "-c", "icns", str(iconset_dir), "-o", str(staged_dir / "icon.icns")],
            step="iconutil 封装 icon.icns",
        )

        # 4. 生成 Tauri 所需的全部 RGBA PNG 图标
        png_specs = [
            ("icon.png", 512),
            ("32x32.png", 32),
            ("128x128.png", 128),
            ("128x128@2x.png", 256),
            ("StoreLogo.png", 50),
            ("Square30x30Logo.png", 30),
            ("Square44x44Logo.png", 44),
            ("Square71x71Logo.png", 71),
            ("Square89x89Logo.png", 89),
            ("Square107x107Logo.png", 107),
            ("Square142x142Logo.png", 142),
            ("Square150x150Logo.png", 150),
            ("Square284x284Logo.png", 284),
            ("Square310x310Logo.png", 310),
        ]
        for fname, sz in png_specs:
            save_rgba_png(image_for_size(sz), staged_dir / fname)

        # 5. 生成多分辨率 RGBA icon.ico（含 16/24/32/48/64/128/256）
        ico_sizes = [16, 24, 32, 48, 64, 128, 256]
        ico_images = [image_for_size(sz) for sz in ico_sizes]
        ico_images[-1].save(
            staged_dir / "icon.ico",
            format="ICO",
            sizes=[(im.width, im.height) for im in ico_images],
            append_images=ico_images[:-1],
        )

        # 6. 全部生成成功后统一原子替换写入 ICONS_DIR
        for src in sorted(staged_dir.iterdir()):
            tmp_target = ICONS_DIR / f".{src.name}.tmp.{os.getpid()}"
            try:
                shutil.copy2(src, tmp_target)
                os.replace(tmp_target, ICONS_DIR / src.name)
            except BaseException:
                tmp_target.unlink(missing_ok=True)
                raise

    print(f"已生成全套 SENSE 图标资源：{ICONS_DIR}")


if __name__ == "__main__":
    main()
