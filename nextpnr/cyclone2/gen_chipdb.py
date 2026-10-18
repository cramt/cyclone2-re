#!/usr/bin/env python3
"""Turn `cram chipdb` output into a himbaechel .bba with nextpnr's own
generator (himbaechel_dbgen). The text is already flat (one line per wire,
pip, bel, tile instance and inter-tile node), so this is a transcription.

    gen_chipdb.py <dbgen dir> <constids.inc> <chipdb.txt[.xz]> <out.bba>

The cyclone2-re repo commits it xz-compressed as
db/EP2C35F672C6/chipdb.txt.xz, so building needs Python and that file only.
"""
import lzma
import sys

sys.path.insert(0, sys.argv[1])
from himbaechel_dbgen.chip import Chip, NodeWire, PinType  # noqa: E402

constids, src, out = sys.argv[2:5]
ch = tt = bel = pkg = None
with (lzma.open(src, "rt") if src.endswith(".xz") else open(src)) as f:
    for line in f:
        a = line.split()
        k = a[0]
        if k == "p":
            tt.create_pip(a[1], a[2])
        elif k == "w":
            tt.create_wire(a[1], a[2])
        elif k == "node":
            ch.add_node([NodeWire(int(a[i]), int(a[i + 1]), a[i + 2]) for i in range(1, len(a), 3)])
        elif k == "t":
            ch.set_tile_type(int(a[1]), int(a[2]), a[3])
        elif k == "tt":
            tt = ch.create_tile_type(a[1])
        elif k == "b":
            bel = tt.create_bel(a[1], a[2], int(a[3]))
        elif k == "bp":
            tt.add_bel_pin(bel, a[1], a[2], PinType.OUTPUT if a[3] == "O" else PinType.INPUT)
        elif k == "end":
            tt = None
        elif k == "chip":
            ch = Chip("cyclone2", "EP2C35F672C6", int(a[1]), int(a[2]))
            ch.strs.read_constids(constids)
            ch.set_speed_grades(["C6"])
        elif k == "pkg":
            pkg = ch.create_package(a[1])
        elif k == "pad":
            pkg.create_pad(a[1], a[2], a[3], "", int(a[4]))
        else:
            raise ValueError(f"unknown line {line!r}")
ch.write_bba(out)
