"""生成 Kada 的托盘图标：荧光 K。

用法（需要 Pillow：`pip install pillow`）：

    python scripts/make-tray-icon.py

产物（覆盖写）：
    src-tauri/icons/tray.png        32x32  Windows / Linux
    src-tauri/icons/tray-macos.png  44x44  macOS（菜单栏上限 22pt，按 2x 给）

设计：在 1024px 画布上画一根圆头荧光管（竖笔 + 两笔斜臂），叠三层光
（宽光晕 / 紧光晕 / 近白热芯），再加一圈很淡的深色边，保证浅色任务栏上也
看得见。最后 Lanczos 缩到目标尺寸。改完图不用手动清理，`lib.rs` 里的
`include_bytes!` 会让 cargo 重编（见 TRAY_ICON 注释）。
"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

S = 4                 # 超采样倍数
N = 64 * S            # 工作画布
DEST = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"


def sc(v: float) -> float:
    """把 256 网格坐标换算到工作画布。"""
    return v * N / 256.0


LW = sc(30)                       # 荧光管粗细
TOP, BOT = sc(24), sc(232)
STEM_X = sc(70)
MID_Y = (TOP + BOT) / 2
ARM_X = sc(190)

mask = Image.new("L", (N, N), 0)
draw = ImageDraw.Draw(mask)


def tube(points, width: float):
    draw.line(points, fill=255, width=int(round(width)), joint="curve")
    r = width / 2
    for x, y in (points[0], points[-1]):
        draw.ellipse([x - r, y - r, x + r, y + r], fill=255)


# 竖笔 + 两笔斜臂；三根管在竖笔中点交汇，那里补一个圆盘让接缝平滑。
tube([(STEM_X, TOP), (STEM_X, BOT)], LW)
tube([(STEM_X, MID_Y), (ARM_X, TOP)], LW)
tube([(STEM_X, MID_Y), (ARM_X, BOT)], LW)
r = LW / 2
draw.ellipse([STEM_X - r, MID_Y - r, STEM_X + r, MID_Y + r], fill=255)


def tinted(alpha: Image.Image, rgb: tuple[int, int, int]) -> Image.Image:
    layer = Image.new("RGBA", (N, N), rgb + (0,))
    layer.putalpha(alpha)
    return layer


base = Image.new("RGBA", (N, N), (0, 0, 0, 0))
# 宽光晕（外圈淡青）+ 紧光晕（内圈亮青）。
halo = mask.filter(ImageFilter.GaussianBlur(sc(12)))
base = Image.alpha_composite(base, tinted(halo.point(lambda v: int(v * 0.55)), (31, 184, 255)))
tight = mask.filter(ImageFilter.GaussianBlur(sc(5)))
base = Image.alpha_composite(base, tinted(tight.point(lambda v: int(v * 0.85)), (79, 225, 255)))
# 深色薄边：浅色任务栏上兜住轮廓。
rim = mask.filter(ImageFilter.GaussianBlur(sc(2.0)))
base = Image.alpha_composite(base, tinted(rim.point(lambda v: 190 if v > 40 else 0), (2, 6, 12)))
# 近白热芯：比管子略细，读起来像发光的灯丝。
core = Image.new("L", (N, N), 0)
dcore = ImageDraw.Draw(core)


def core_tube(points, width: float):
    dcore.line(points, fill=255, width=int(round(width)), joint="curve")
    rr = width / 2
    for x, y in (points[0], points[-1]):
        dcore.ellipse([x - rr, y - rr, x + rr, y + rr], fill=255)


CW = LW * 0.52
inset = LW * 0.18
core_tube([(STEM_X, TOP + inset), (STEM_X, BOT - inset)], CW)
core_tube([(STEM_X, MID_Y), (ARM_X - LW * 0.10, TOP + inset)], CW)
core_tube([(STEM_X, MID_Y), (ARM_X - LW * 0.10, BOT - inset)], CW)
base = Image.alpha_composite(base, tinted(core, (232, 255, 255)))

DEST.mkdir(parents=True, exist_ok=True)
for size, name in ((32, "tray.png"), (44, "tray-macos.png")):
    base.resize((size, size), Image.LANCZOS).save(DEST / name)
    print(f"wrote {DEST / name} ({size}x{size})")
