#!/usr/bin/env bash
# Four solid-color 2000x2000 PNGs: each estimates 2000*2000/750 ~= 5334
# tokens by geometry, ~21k tokens of image payload in the first message.
set -euo pipefail
mkdir -p "$HOME/.prime/agent"
cat > "$HOME/.prime/agent/settings.json" <<'EOF'
{"compaction": {"enabled": true, "keepRecentTokens": 4000, "reserveTokens": 2000}}
EOF
python3 - <<'PY'
import struct
import zlib


def solid_png(path, width=2000, height=2000, rgb=(90, 120, 200)):
    raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))

    def chunk(tag, data):
        body = struct.pack(">I", len(data)) + tag + data
        return body + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    signature = bytes((137, 80, 78, 71, 13, 10, 26, 10))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    png = signature + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    with open(path, "wb") as fh:
        fh.write(png)


for i in range(1, 5):
    solid_png(f"img{i}.png", rgb=(40 * i, 100, 200))
print("images ready")
PY
