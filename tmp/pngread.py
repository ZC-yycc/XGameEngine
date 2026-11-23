"""最小 PNG 解码（stdlib only）：8 位 RGB/RGBA、无隔行。只读，不改仓库。"""
import struct
import zlib


def _paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    return b if pb <= pc else c


def load_png(path):
    data = open(path, "rb").read()
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    pos = 8
    idat = b""
    w = h = color = depth = 0
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        ctype = data[pos + 4 : pos + 8]
        chunk = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if ctype == b"IHDR":
            w, h, depth, color, comp, filt, interlace = struct.unpack(">IIBBBBB", chunk)
            assert depth == 8 and comp == 0 and filt == 0 and interlace == 0, (depth, comp, filt, interlace)
        elif ctype == b"IDAT":
            idat += chunk
        elif ctype == b"IEND":
            break
    assert color in (2, 6), color
    bpp = 3 if color == 2 else 4
    raw = zlib.decompress(idat)
    stride = w * bpp
    out = bytearray(h * stride)
    prev = bytearray(stride)
    p = 0
    for y in range(h):
        ft = raw[p]
        p += 1
        line = bytearray(raw[p : p + stride])
        p += stride
        if ft == 1:
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 0xFF
        elif ft == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif ft == 3:
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif ft == 4:
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                upleft = prev[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + _paeth(left, prev[i], upleft)) & 0xFF
        else:
            assert ft == 0, ft
        out[y * stride : (y + 1) * stride] = line
        prev = line
    return w, h, bpp, bytes(out)


def load_ppm(path):
    data = open(path, "rb").read()
    assert data[:2] == b"P6"
    fields = []
    pos = 2
    while len(fields) < 3:
        while data[pos : pos + 1].isspace():
            pos += 1
        start = pos
        while not data[pos : pos + 1].isspace():
            pos += 1
        fields.append(int(data[start:pos]))
    pos += 1
    w, h, _maxv = fields
    body = data[pos : pos + w * h * 3]
    return w, h, 3, body


def pixels(w, h, bpp, body, x, y):
    i = (y * w + x) * bpp
    return list(body[i : i + 3])


def region_mean(w, h, bpp, body, x0, y0, x1, y1):
    n = 0
    acc = [0, 0, 0]
    for y in range(y0, y1):
        for x in range(x0, x1):
            i = (y * w + x) * bpp
            for c in range(3):
                acc[c] += body[i + c]
            n += 1
    return [round(a / max(n, 1), 2) for a in acc]
