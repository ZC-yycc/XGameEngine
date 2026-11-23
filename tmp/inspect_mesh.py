"""对照 old-150-now.png 与 new150-mesh.ppm：区域结构性 + 球 / 狐狸扫描线。只读。"""
from pngread import load_png, load_ppm

w0, h0, b0, old = load_png("../tmp/old-150-now.png")
w1, h1, b1, new = load_ppm("../tmp/new150-mesh.ppm")
assert (w0, h0) == (w1, h1) and (w0, h0) == (1280, 720)


def px(buf, bpp, x, y):
    i = (y * w0 + x) * bpp
    return tuple(buf[i : i + 3])


def region(name, x0, y0, x1, y1):
    n = 0
    big = 0
    so = [0, 0, 0]
    sn = [0, 0, 0]
    for y in range(y0, y1):
        for x in range(x0, x1):
            o = px(old, b0, x, y)
            m = px(new, b1, x, y)
            d = max(abs(o[c] - m[c]) for c in range(3))
            big += d >= 8
            for c in range(3):
                so[c] += o[c]
                sn[c] += m[c]
            n += 1
    print(f"[{name}] n={n} old_mean={[round(v / n, 2) for v in so]} new_mean={[round(v / n, 2) for v in sn]} structural={big} ({big * 100.0 / n:.2f}%)")


region("sphere", 1050, 300, 1100, 350)
region("sphere-wide", 1020, 270, 1160, 420)
region("fox", 600, 420, 660, 470)
region("red-boxes", 400, 160, 480, 240)

print("\n--- sphere 横线 y=330 ---")
for x in range(1030, 1171, 10):
    print(f"  x={x}: old={px(old, b0, x, 330)} new={px(new, b1, x, 330)}")
print("\n--- sphere 竖线 x=1090 ---")
for y in range(280, 401, 10):
    print(f"  y={y}: old={px(old, b0, 1090, y)} new={px(new, b1, 1090, y)}")
