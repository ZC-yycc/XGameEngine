"""分析 old-150.png 与 new-150.ppm 的逐像素差异分布。只读，不改仓库。"""
import sys
from pathlib import Path

import numpy as np
from PIL import Image


def load_ppm(path: Path) -> np.ndarray:
    data = path.read_bytes()
    # P6\n<w> <h>\n255\n
    assert data[:2] == b"P6"
    fields = []
    pos = 2
    while len(fields) < 3:
        while data[pos : pos + 1].isspace():
            pos += 1
        if data[pos : pos + 1] == b"#":
            while data[pos : pos + 1] != b"\n":
                pos += 1
            continue
        start = pos
        while not data[pos : pos + 1].isspace():
            pos += 1
        fields.append(int(data[start:pos]))
    pos += 1
    w, h, _maxv = fields
    return np.frombuffer(data[pos : pos + w * h * 3], dtype=np.uint8).reshape(h, w, 3)


def load_png(path: Path) -> np.ndarray:
    img = Image.open(path)
    if img.mode == "RGBA":
        # PPM 无 alpha；比较时只看 RGB（xgc image-diff 含 alpha，但旧 PNG alpha 应全 255）
        return np.asarray(img.convert("RGB"), dtype=np.uint8)
    return np.asarray(img.convert("RGB"), dtype=np.uint8)


def main() -> None:
    base = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
    old = load_png(base / "old-150.png")
    new = load_ppm(base / "new-150.ppm")
    assert old.shape == new.shape, (old.shape, new.shape)
    h, w, _ = old.shape
    diff = np.abs(old.astype(np.int16) - new.astype(np.int16))
    maxd = diff.max(axis=2)
    changed = maxd > 0
    print(f"size={w}x{h} changed={changed.sum()}/{h*w} ({changed.mean()*100:.2f}%)")
    for lo, hi, name in [(0, 0, "same"), (1, 1, "1"), (2, 3, "2-3"), (4, 7, "4-7"), (8, 255, ">=8")]:
        sel = (maxd >= lo) & (maxd <= hi)
        print(f"  bucket {name}: {sel.sum()} ({sel.mean()*100:.3f}%)")

    # ±1 像素的分布：按行 / 按列直方图，以及和图像梯度的关系
    one = maxd == 1
    rows = one.sum(axis=1)
    cols = one.sum(axis=0)
    print(f"  ±1 像素行范围: " + ", ".join(f"r{i}={int(rows[i])}" for i in range(0, h, 80)))
    print(f"  ±1 像素列范围: " + ", ".join(f"c{i}={int(cols[i])}" for i in range(0, w, 160)))

    # 平坦区域判定：old 图 3x3 邻域内最大通道差 <=1 的像素视为平坦
    pad = np.pad(old, ((1, 1), (1, 1), (0, 0)), mode="edge")
    flat = np.ones((h, w), dtype=bool)
    for dy in (0, 1, 2):
        for dx in (0, 1, 2):
            nb = pad[dy : dy + h, dx : dx + w, :]
            flat &= np.abs(nb.astype(np.int16) - old.astype(np.int16)).max(axis=2) <= 1
    print(f"  平坦像素 {flat.sum()}（{flat.mean()*100:.2f}%），其中 changed={changed[flat].sum()}")
    print(f"  平坦像素里 ±1: {one[flat].sum()}，结构性(>=8): {(maxd[flat] >= 8).sum()}")
    print(f"  非平坦像素 changed={changed[~flat].sum()}，其中 ±1={one[~flat].sum()}，>=8={(maxd[~flat] >= 8).sum()}")

    # 展示前 20 个结构性差异最大像素位置
    ys, xs = np.nonzero(maxd >= 8)
    order = np.argsort(maxd[ys, xs])[::-1][:10]
    print("  最差像素（y,x,maxd,old,new）:")
    for k in order:
        y, x = int(ys[k]), int(xs[k])
        print(f"    ({y},{x}) maxd={int(maxd[y,x])} old={old[y,x].tolist()} new={new[y,x].tolist()}")


if __name__ == "__main__":
    main()
