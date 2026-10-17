#!/usr/bin/env python3
# Strip the Quartus .ddb header and unpack every XZ stream found in it.
import lzma, pathlib, sys

XZ_MAGIC = b"\xfd7zXZ\x00"
src = pathlib.Path(sys.argv[1])
out_dir = pathlib.Path(sys.argv[2])
out_dir.mkdir(parents=True, exist_ok=True)
data = src.read_bytes()

pos, n = 0, 0
while (pos := data.find(XZ_MAGIC, pos)) != -1:
    d = lzma.LZMADecompressor(lzma.FORMAT_XZ)
    try:
        payload = d.decompress(data[pos:])
    except lzma.LZMAError as e:
        print(f"{src.name} @{pos}: {e}")
        pos += 1
        continue
    consumed = len(data) - pos - len(d.unused_data)
    dst = out_dir / f"{src.stem}.{n}.bin"
    dst.write_bytes(payload)
    print(f"{src.name} @{pos}: {consumed} -> {len(payload)} bytes -> {dst.name}")
    pos += consumed
    n += 1
print(f"{src.name}: {n} streams, {len(data)} bytes total")
