"""对照 old-150-now.png 与 new-150.ppm 的具体区域，打印逐点 RGB / 差值。只读。"""
from pngread import load_png, load_ppm

w0, h0, b0, old = load_png("tmp/old-150-now.png")
w1, h1, b1, new = load_ppm("tmp/new-150.ppm")
assert (w0, h0) == (w1, h1) and (w0, h0) == (1280, 720)


def px(buf, bpp, x, y):
    i = (y * w0 + x) * bpp
    return tuple(buf[i : i + 3])


def region(name, x0, y0, x1, y1):
    n = 0
    so = [0, 0, 0]
    sn = [0, 0, 0]
    big = 0
    worst = (0, None)
    for y in range(y0, y1):
        for x in range(x0, x1):
            o = px(old, b0, x, y)
            n_ = px(new, b1, x, y)
            d = max(abs(o[c] - n_[c]) for c in range(3))
            if d >= 8:
                big += 1
            if d > worst[0]:
                worst = (d, (x, y, o, n_))
            for c in range(3):
                so[c] += o[c]
                sn[c] += n_[c]
            n += 1
    print(f"[{name}] x={x0}..{x1} y={y0}..{y1} n={n}")
    print(f"  old mean={[round(v/n,2) for v in so]}  new mean={[round(v/n,2) for v in sn]}")
    print(f"  structural(>=8)={big} ({big*100.0/n:.2f}%)  maxdiff={worst[0]} at {worst[1]}")


# 区域：红方块堆 / 狐狸 / 红球 / 蓝箱 / 地面
region("red-boxes", 400, 160, 480, 240)
region("fox", 600, 420, 660, 470)
region("sphere", 1050, 300, 1100, 350)
region("blue-box", 720, 280, 800, 350)
region("ground", 300, 600, 400, 650)

# 红方块：取一条竖线，看像素是不是整块面都不同
print("\n--- 红方块竖线 x=430, y=170..230 (old -> new) ---")
for y in range(170, 231, 5):
    print(f"  y={y}: old={px(old,b0,430,y)} new={px(new,b1,430,y)}")

print("\n--- 红球横线 y=310, x=1050..1100 ---")
for x in range(1050, 1101, 10):
    print(f"  x={x}: old={px(old,b0,x,310)} new={px(new,b1,x,310)}")

print("\n--- 狐狸横线 y=440, x=590..660 ---")
for x in range(590, 661, 10):
    print(f"  x={x}: old={px(old,b0,x,440)} new={px(new,b1,x,440)}")
