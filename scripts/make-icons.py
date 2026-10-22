"""生成 Kada 的托盘图标与应用图标：品牌字标里的那枚青蓝荧光 K。

用法（需要 Pillow + fonttools + brotli：`pip install pillow fonttools brotli`）：

    python scripts/make-icons.py          # 只出图标资产
    npm run tauri -- icon src-tauri/icons/app-icon.png   # 再展开成平台图标集

产物（覆盖写）：
    src-tauri/icons/tray.png        32x32  Windows / Linux 托盘
    src-tauri/icons/tray-macos.png  44x44  macOS（菜单栏上限 22pt，按 2x 给）
    src-tauri/icons/app-icon.png    1024x1024  应用图标母版（交给 `tauri icon` 展开）

字形就是 logo 里的 K：直接读 `ui/src/assets/fonts/kada-pixel-sc.woff2`
（Kada Pixel = Fusion Pixel 12px Monospaced，OFL 1.1），取 'K' 的轮廓，
所以托盘 / 应用图标 / 品牌字标三处是同一个字形，改字体即一起改。

配色同 logo：字心 `#63e8ff`（CRT 主题的 `--crt-cyan`），外面两圈同色辉光。
托盘另加一圈很淡的深色薄边——浅色任务栏上兜住轮廓（透明辉光会被浅底吃掉）。
应用图标保留原有深色方块（外圈 `#181a20` + 内屏 `#343a48`），只换字心。

改完图不用手动清理：`lib.rs` 里的 `include_bytes!` 会让 cargo 重编（见 TRAY_ICON 注释）。
"""
from pathlib import Path
from tempfile import TemporaryDirectory

from fontTools.ttLib import TTFont
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent.parent
DEST = ROOT / "src-tauri" / "icons"
FONT = ROOT / "ui" / "src" / "assets" / "fonts" / "kada-pixel-sc.woff2"

INK = (99, 232, 255)        # #63e8ff  logo 的 --crt-cyan，字心颜色
GLOW = (99, 232, 255)       # 外圈辉光同色（logo 的 text-shadow 就是同一支荧光粉）
GLOW_HI = (150, 243, 255)   # 近圈略提亮，让笔画边缘更「热」
RIM_COLOR = (4, 16, 24)     # 浅底兜底用的薄边

TILE_OUTER = (24, 26, 32)   # #181a20  应用图标外圈
TILE_INNER = (52, 58, 72)   # #343a48  应用图标内屏
TILE_BORDER = 140           # 内屏四边留白（1024 画布上的绝对像素，沿原图标）

# 辉光 / 薄边参数，都按「字高 H 的比例」给，换尺寸不用改。
HALO_WIDE = (0.16, 0.70)    # 远光：半径、峰值不透明度
HALO_TIGHT = (0.05, 0.90)   # 近光：半径、峰值不透明度
RIM = (0.06, 200, 0.9)      # 薄边：半径、阈值后不透明度、整体强度


def glyph_mask(char: str = "K", em: int = 1200) -> Image.Image:
    """把字符从 woff2 里渲染成裁到 bbox 的灰度蒙版（pixel 字体按轮廓缩放）。"""
    with TemporaryDirectory() as tmp:
        ttf = Path(tmp) / "kada-pixel.ttf"
        font = TTFont(FONT)
        font.flavor = None  # woff2 -> 裸 sfnt，Pillow 只吃 ttf
        font.save(str(ttf))
        pil_font = ImageFont.truetype(str(ttf), em)
        canvas = Image.new("L", (em * 2, em * 2), 0)
        ImageDraw.Draw(canvas).text((em // 2, em // 2), char, font=pil_font, fill=255)
    return canvas.crop(canvas.getbbox())


def scaled_glyph(mask: Image.Image, height: float) -> Image.Image:
    """按目标字高等比缩放蒙版。"""
    width = round(mask.width * height / mask.height)
    return mask.resize((max(1, width), max(1, round(height))), Image.LANCZOS)


def tinted(alpha: Image.Image, rgb: tuple[int, int, int], strength: float = 1.0) -> Image.Image:
    layer = Image.new("RGBA", alpha.size, rgb + (0,))
    if strength < 1.0:
        alpha = alpha.point(lambda v: int(v * strength))
    layer.putalpha(alpha)
    return layer


def compose(mask: Image.Image, *, rim: bool, size: int, box: int) -> Image.Image:
    """把字心 + 两圈辉光（+ 可选薄边）画进 box×box 透明画布并缩到 size。"""

    def at_center(img: Image.Image) -> Image.Image:
        canvas = Image.new("RGBA", (box, box), (0, 0, 0, 0))
        canvas.alpha_composite(img, ((box - img.width) // 2, (box - img.height) // 2))
        return canvas

    h = mask.height
    wide_r, wide_a = HALO_WIDE
    tight_r, tight_a = HALO_TIGHT
    out = at_center(tinted(mask.filter(ImageFilter.GaussianBlur(h * wide_r)), GLOW, wide_a))
    out = Image.alpha_composite(out, at_center(tinted(mask.filter(ImageFilter.GaussianBlur(h * tight_r)), GLOW_HI, tight_a)))
    if rim:
        rim_r, rim_a, rim_k = RIM
        edge = mask.filter(ImageFilter.GaussianBlur(h * rim_r)).point(lambda v: rim_a if v > 32 else 0)
        out = Image.alpha_composite(out, at_center(tinted(edge, RIM_COLOR, rim_k)))
    out = Image.alpha_composite(out, at_center(tinted(mask, INK)))
    return out.resize((size, size), Image.LANCZOS) if size != box else out


def main() -> None:
    glyph = glyph_mask()
    DEST.mkdir(parents=True, exist_ok=True)

    # ---- 托盘：透明底，字高约画布 70%，辉光按字高等比 ----
    for box, k_ratio, size, name in (
        (1024, 0.70, 32, "tray.png"),
        (1408, 0.70, 44, "tray-macos.png"),
    ):
        k = scaled_glyph(glyph, box * k_ratio)
        img = compose(k, rim=True, size=size, box=box)
        img.save(DEST / name)
        print(f"wrote {DEST / name} ({size}x{size})")

    # ---- 应用图标：深色方块 + 同一个字心 ----
    app = Image.new("RGBA", (1024, 1024), TILE_OUTER + (255,))
    ImageDraw.Draw(app).rectangle(
        [TILE_BORDER, TILE_BORDER, 1024 - TILE_BORDER - 1, 1024 - TILE_BORDER - 1],
        fill=TILE_INNER + (255,),
    )
    # 底板直接按 1024 画（方块边缘保持硬边）；只有字心在 2048 上超采样后缩下来。
    box = 2048
    k = scaled_glyph(glyph, box * 0.38)
    art = compose(k, rim=False, size=1024, box=box)
    app = Image.alpha_composite(app, art)
    app.convert("RGB").save(DEST / "app-icon.png")
    print(f"wrote {DEST / 'app-icon.png'} (1024x1024)")


if __name__ == "__main__":
    main()
