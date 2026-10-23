#!/usr/bin/env python3
# 由品牌图生成浏览器扩展图标（16/32/48/128）。
#
# 桌面应用图标是米白圆角方块 + 深色鸟剪影（hawk-app/build/icon.png）；直接缩到 16px 塞进浏览器
# 工具栏，方块背景会吃掉大半像素，看起来比其他扩展小一圈。所以扩展图标改成：
#   - 透明底，只留鸟形（顺带去掉原图底部的支撑横线，它占了整幅宽度的 1/6 却没信息量）
#   - 鸟按「较长边占画布 90%」放大并居中，工具栏里与相邻图标视觉等大
#   - 抗锯齿边按覆盖率换算 alpha；透明像素的 RGB 填前景色，缩放插值不会渗出背景色
#
# 用法：python3 tools/gen-extension-icons.py
# 产物：hawk-browser-extension/public/icons/{16,32,48,128}.png（RGBA，透明底）
import math
import pathlib
import sys

from PIL import Image

REPO = pathlib.Path(__file__).resolve().parents[1]
SOURCE = REPO / 'hawk-app' / 'build' / 'icon.png'
OUT_DIR = REPO / 'hawk-browser-extension' / 'public' / 'icons'
SIZES = (16, 32, 48, 128)
# 鸟的较长边占画布的��例：0.9 → 16px 图标里鸟约 14px，比原图（约 8px）大一圈
FILL_RATIO = 0.9


def luminance(rgb: tuple[int, int, int]) -> float:
    return 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2]


def distance(a: tuple[int, int, int], b: tuple[int, int, int]) -> float:
    return math.dist(a, b)


def detect_backdrop(im: Image.Image) -> tuple[int, int, int]:
    """背景色取四角里最常见的那种（圆角图四角是透明的，取 alpha 最大的角落像素区）"""
    w, h = im.size
    px = im.load()
    for probe in ((2, 2), (w - 3, 2), (2, h - 3), (w - 3, h - 3), (w // 2, 2), (2, h // 2)):
        pixel = px[probe]
        if pixel[3] > 200:
            return pixel[:3]
    raise SystemExit('找不到不透明的背景色（源图四个角都是透明的）')


def detect_figure_color(im: Image.Image, backdrop: tuple[int, int, int]) -> tuple[int, int, int]:
    """前景色取离背景最远的像素（鸟的深灰）"""
    best = None
    best_dist = -1.0
    rgb = im.convert('RGB')
    pixels = rgb.get_flattened_data() if hasattr(rgb, 'get_flattened_data') else rgb.getdata()
    for pixel in pixels:
        d = distance(pixel, backdrop)
        if d > best_dist:
            best, best_dist = pixel, d
    return best


def support_line_rows(im: Image.Image, backdrop: tuple[int, int, int]) -> set[int]:
    """找出原图底部那条贯穿左右的支撑横线所在行（宽度恒定、位于下半部）。

    直接按亮度切会让线混在鸟的包围盒里，把裁剪框撑到整幅宽度，鸟就永远放不大。
    """
    w, h = im.size
    px = im.load()
    widths: dict[int, int] = {}
    for y in range(h):
        count = 0
        for x in range(w):
            pixel = px[x, y]
            if pixel[3] > 128 and distance(pixel[:3], backdrop) > 60:
                count += 1
        widths[y] = count

    widest = max(widths.values())
    # 线的特征：行宽接近整幅里的最大值且连续多行毫无变化（鸟的每行宽度都在变）
    rows: set[int] = set()
    for y in range(h // 2, h):
        if widths[y] < widest * 0.9:
            continue
        same = [y]
        while same[0] - 1 in widths and widths[same[0] - 1] == widths[y]:
            same.insert(0, same[0] - 1)
        while same[-1] + 1 in widths and widths[same[-1] + 1] == widths[y]:
            same.append(same[-1] + 1)
        if len(same) >= 4:
            rows.update(same)
    return rows


def to_transparent(im: Image.Image) -> Image.Image:
    """底色抠成透明：按与背景的色差换算 alpha，并对半透明边做反预乘，避免留下白色毛边。

    底部支撑线先整段裁掉（含上下抗锯齿各两行）——否则它会把包围盒撑到整幅宽度。
    """
    line_rows = support_line_rows(im, detect_backdrop(im))
    if line_rows:
        cut = max(0, min(line_rows) - 2)
        im = im.crop((0, 0, im.size[0], cut))
    backdrop = detect_backdrop(im)
    figure = detect_figure_color(im, backdrop)
    span = distance(figure, backdrop)
    low, high = span * 0.18, span * 0.55  # 低于 low 视为背景，高于 high 视为实心（橙色眼睛因此不会被削半透明）

    out = Image.new('RGBA', im.size)
    src = im.load()
    dst = out.load()
    for y in range(im.size[1]):
        for x in range(im.size[0]):
            pixel = src[x, y]
            if pixel[3] < 16:
                dst[x, y] = (*figure, 0)
                continue
            d = distance(pixel[:3], backdrop)
            alpha = 0.0 if d <= low else 1.0 if d >= high else (d - low) / (high - low)
            if alpha <= 0.0:
                dst[x, y] = (*figure, 0)
                continue
            a8 = round(alpha * 255)
            # 反预乘：像素本是 figure 与 backdrop 的混合，还原成纯前景色
            rgb = tuple(
                min(255, max(0, round((pixel[i] - backdrop[i] * (1 - alpha)) / alpha))) for i in range(3)
            )
            dst[x, y] = (*rgb, a8)
    return out


def render(figure: Image.Image, size: int) -> Image.Image:
    """按较长边占 FILL_RATIO 缩放并居中贴到方形透明画布"""
    content = max(1, round(size * FILL_RATIO))
    w, h = figure.size
    scale = content / max(w, h)
    resized = figure.resize((max(1, round(w * scale)), max(1, round(h * scale))), Image.LANCZOS)
    canvas = Image.new('RGBA', (size, size), (0, 0, 0, 0))
    canvas.paste(resized, ((size - resized.size[0]) // 2, (size - resized.size[1]) // 2), resized)
    return canvas


def main() -> int:
    if not SOURCE.is_file():
        raise SystemExit(f'源图不存在: {SOURCE}')
    source = Image.open(SOURCE).convert('RGBA')
    transparent = to_transparent(source)
    # 只看 alpha 通道：透明像素的 RGB 被填成前景色（防缩放渗色），按 RGB 求包围盒会把它算成内容
    alpha = transparent.getchannel('A').point(lambda v: 255 if v > 8 else 0)
    bbox = alpha.getbbox()
    if bbox is None:
        raise SystemExit('抠图后没有内容，检查源图与背景色判定')
    figure = transparent.crop(bbox)
    print(f'源图 {source.size}，去背景后鸟的包围盒 {bbox} → {figure.size}')

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for size in SIZES:
        target = OUT_DIR / f'{size}.png'
        render(figure, size).save(target)
        print(f'  {target.relative_to(REPO)}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
