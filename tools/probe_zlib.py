#!/usr/bin/env python3
# Find offsets in a .ddb where a zlib or raw-deflate stream decodes cleanly.
import sys, zlib

data = open(sys.argv[1], "rb").read()
limit = int(sys.argv[2]) if len(sys.argv) > 2 else 4096
hits = []
for off in range(min(limit, len(data))):
    for wbits, kind in ((15, "zlib"), (-15, "raw")):
        d = zlib.decompressobj(wbits)
        try:
            out = d.decompress(data[off:off + 1 << 20], 1 << 16)
        except zlib.error:
            continue
        if len(out) >= 64:
            hits.append((off, kind, len(out), out[:48]))
for h in hits[:20]:
    print(h)
print("total hits", len(hits))
