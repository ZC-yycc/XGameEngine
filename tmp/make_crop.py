"""把 old/new 两图的同一区域并排放大成 PNG（左旧右新）。只读源图，只写 tmp 下的对照图。

用法：python tmp/make_crop.py x0 y0 x1 y1 scale old.png new.ppm out.png
"""
import struct
import sys
import zlib

from pngread import load_png, load_ppm


def write_png(path, w, h, body):
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        raw += body[y * w * 3 : (y + 1) * w * 3]

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    open(path, "wb").write(png)


def region(body, bpp, x0, y0, x1, y1):
    rows = []
    for y in range(y0, y1):
        row = bytearray()
        for x in range(x0, x1):
            i = (y * w_src + x) * bpp
            row += body[i : i + 3]
        rows.append(bytes(row))
    return rows


x0, y0, x1, y1, scale = (int(v) for v in sys.argv[1:6])
old_path, new_path, out_path = sys.argv[6:9]
w_src, h_src, b_old, old = load_png(old_path)
_, _, b_new, new = load_ppm(new_path)
assert (w_src, h_src) == (1280, 720)

rows_old = region(old, b_old, x0, y0, x1, y1)
rows_new = region(new, b_new, x0, y0, x1, y1)
cw, ch = x1 - x0, y1 - y0
gap = 4
out_w = (cw * 2 + gap) * scale
out_h = ch * scale
out = bytearray()
for row_old, row_new in zip(rows_old, rows_new):
    line = bytearray()
    for sx in range(cw):
        for px in (row_old, row_new):
            r, g, b = px[sx * 3 : sx * 3 + 3]
            line += bytes([r, g, b]) * scale
        line += b"\x00" * 0 if False else b""
    # 中间黑缝
    merged = bytearray()
    for sx in range(cw):
        r, g, b = row_old[sx * 3 : sx * 3 + 3]
        merged += bytes([r, g, b]) * scale
    merged += b"\x00\x00\x00" * (gap * scale)
    for sx in range(cw):
        r, g, b = row_new[sx * 3 : sx * 3 + 3]
        merged += bytes([r, g, b]) * scale
    for _ in range(scale):
        out += merged
write_png(out_path, out_w, out_h, out)
print(f"{out_path}: {out_w}x{out_h}")
