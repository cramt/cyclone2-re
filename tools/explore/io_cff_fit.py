"""Fit the CFF bit -> replicated-frame bit map (tools/cram/src/io.rs
CFF_LANES) from the unused-pin pull-up design.

    ddb asmdb-json <devinfo>/ddb_magellan4_asmdb.ddb > /tmp/asmdb.json
    ddb pins <devinfo> magellan4 fbga672 > /tmp/pins.csv
    python3 -I tools/explore/io_cff_fit.py /tmp/asmdb.json /tmp/pins.csv \\
        specimens/route/empty/build/output_files/top.rbf \\
        specimens/io/unused4/build/output_files/top.rbf

With "AS INPUT TRI-STATED WITH WEAK PULL-UP" every unused IOE gets its
`IO_USE_PULL_UP` CFF bit set, so the replicated-frame bits that flip
against the empty design are exactly those bits (the dual-purpose pins and
the empty design's one output excepted). Ordering both sides gives one
(lane, offset) per run of 24-bit CFF blocks. The dedicated clock inputs
(not pulled up) were placed afterwards from the I/O fuzz designs.
"""
import csv, json, sys

IO_USE_PULL_UP = 176
HDR, FB = 0x186, 314

asmdb = json.load(open(sys.argv[1]))
pull_ups = []
for g in asmdb["groups"]:
    if g["type"] == IO_USE_PULL_UP and g["block"] in (5, 6):
        pull_ups += [(g["block"], i, b & 0xFFFFFF) for i, b in enumerate(g["body"]["bits"])]

# Path i of block 5/6 is the i-th Row/Column I/O pad in pad-id order.
rows = sorted((r for r in csv.DictReader(open(sys.argv[2])) if r["pad"]), key=lambda r: int(r["pad"]))
pin_of = {}
for block, kind in ((5, "Row I/O"), (6, "Column I/O")):
    for i, r in enumerate(r for r in rows if r["io_type"] == kind):
        pin_of[(block, i)] = r
# Not pulled up: already pulled up in the empty design (ASDO, nCSO), an
# output (nCEO), the empty design's LED.
skip = {"E3", "D3", "AE24", "AE23"}
ks = sorted(k for b, i, k in pull_ups if pin_of[(b, i)]["pin"] not in skip)


def rep(d, f):
    return bytes(d[HDR + (f * FB + b) * 4] for b in range(FB))


a, b = open(sys.argv[3], "rb").read(), open(sys.argv[4], "rb").read()
flips = []
for f in range(6):
    x, y = rep(a, f), rep(b, f)
    for i in range(312):
        for k in range(8):
            if (x[i] ^ y[i]) >> k & 1:
                # lane k, byte slot u = 311 * frame + byte - 1
                flips.append((7 - k, f * 311 + i - 1))
flips.sort()
assert len(flips) == len(ks), (len(flips), len(ks))
prev = None
for k, (li, u) in zip(ks, flips):
    lane, off = 7 - li, k - u
    if prev != (lane, off):
        print(f"lane {lane}: from block {k // 24}, offset {off}")
        prev = (lane, off)
