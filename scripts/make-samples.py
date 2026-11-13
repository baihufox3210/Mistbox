#!/usr/bin/env python3
"""Regenerate the binary / encoding samples under samples/ (deterministic)."""
import struct, zlib, pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent / "samples"


def png(path, w, h, color_fn):
    raw = b"".join(b"\x00" + b"".join(bytes(color_fn(x, y)) for x in range(w)) for y in range(h))
    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) \
        + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


# Milk-blue bar chart.
def chart(x, y):
    bars = [(20, 60, 140), (80, 120, 90), (140, 180, 160), (200, 240, 60)]
    for x0, x1, top in bars:
        if x0 <= x < x1 and y >= 200 - top:
            return (91, 143, 195)
    return (255, 253, 249) if y < 200 else (44, 92, 140)

png(ROOT / "markdown/img/chart.png", 260, 204, chart)
png(ROOT / "markdown/img/小 圖.png", 48, 48, lambda x, y: (201, 222, 243) if (x // 8 + y // 8) % 2 else (247, 244, 238))
png(ROOT / "latex/ok/figs/plot.png", 200, 120, lambda x, y: (44, 92, 140) if abs(y - (60 - int(40 * __import__('math').sin(x / 20)))) < 3 else (255, 255, 255))
png(ROOT / "misc/photo.png", 64, 64, lambda x, y: (x * 4 % 256, y * 4 % 256, 200))

enc = ROOT / "encoding"
enc.mkdir(parents=True, exist_ok=True)
text = "繁體中文 Big5 編碼測試：雲端硬碟、預覽、編碼偵測。\n第二行：原始位元組必須完整保存。\n"
(enc / "big5-繁體.txt").write_bytes(text.encode("big5"))
(enc / "gbk-简体.txt").write_bytes("简体中文 GBK 编码测试：云端硬盘与编码侦测。\n第二行内容。\n".encode("gbk"))
(enc / "utf16le-bom.txt").write_bytes(b"\xff\xfe" + "UTF-16LE 含 BOM：你好\n".encode("utf-16-le"))
(enc / "latin1.txt").write_bytes("Café, naïve, façade - Latin-1 encoded.\n".encode("latin-1"))

misc = ROOT / "misc"
misc.mkdir(parents=True, exist_ok=True)
(misc / "binary.bin").write_bytes(bytes(range(256)) * 64)
(misc / "no-extension").write_bytes(b"plain text without an extension\n\xe4\xb8\xad\xe6\x96\x87\n")
(misc / ".hidden-file").write_text("hidden file content\n")
(misc / "中文 檔名 含 空白.txt").write_text("檔名包含中文與空白。\n", encoding="utf-8")
(misc / "evil.html").write_text("<!doctype html><script>alert(document.cookie)</script><h1>must not run</h1>\n")
(misc / "evil.svg").write_text('<svg xmlns="http://www.w3.org/2000/svg" width="120" height="40"><script>alert(1)</script>'
                               '<rect width="120" height="40" fill="#c9def3"/><text x="10" y="25">SVG</text></svg>\n')
(misc / "archive.zip").write_bytes(b"PK\x05\x06" + b"\x00" * 18)  # empty zip
print("samples regenerated under", ROOT)
