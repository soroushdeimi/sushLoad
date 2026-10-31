#!/usr/bin/env python3
"""Writes a 1024x1024 PNG app icon (blue rounded square, white down-arrow) using only the stdlib."""
import math
import struct
import sys
import zlib

N = 1024


def pixel(x, y):
    # rounded square mask
    r = 230
    cx = min(max(x, r), N - 1 - r)
    cy = min(max(y, r), N - 1 - r)
    if math.hypot(x - cx, y - cy) > r:
        return (0, 0, 0, 0)
    t = y / N
    base = (int(40 + 0 * t), int(150 - 60 * t), int(255 - 20 * t))
    # arrow: shaft + head + tray
    u, v = x - N / 2, y - N / 2
    shaft = abs(u) < 60 and -250 < v < 60
    head = v >= 20 and v < 220 and abs(u) < (220 - v) * 1.0 + 0 and abs(u) <= 220 - (v - 20) * 1.1
    tray = 230 < v < 290 and abs(u) < 270
    if shaft or head or tray:
        return (255, 255, 255, 255)
    return base + (255,)


def main(out):
    raw = bytearray()
    for y in range(N):
        raw.append(0)
        for x in range(N):
            raw.extend(pixel(x, y))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", N, N, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 6)) + chunk(b"IEND", b"")
    open(out, "wb").write(png)


if __name__ == "__main__":
    main(sys.argv[1])
