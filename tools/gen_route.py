#!/usr/bin/env python3
"""Generate a routing-heavy test design for tools/oracle.sh.

    gen_route.py <out-dir> <seed> [<chains> [<luts per chain>]]

Each chain is SW[i] -> buffer LUT -> buffer LUT -> ... -> LEDR[i], with every
LUT pinned to a random LAB location. Far-apart pins force long C4/R4/C16/R24
runs, which is the point: the more routing per design, the more CRAM bits to
fit the flat-address mapping against. Buffers are WYSIWYG lcell_combs so
synthesis can't collapse the chain.
"""
import pathlib, random, sys

# EP2C35 LAB grid, read off the routing graph (LE output nodes): X 1..64 minus
# the M4K/multiplier columns, Y 1..35.
LAB_X = [x for x in range(1, 65) if x not in (13, 26, 39, 52)]
LAB_Y = list(range(1, 36))

# DE2 user manual pinout.
SW = ["N25", "N26", "P25", "AE14", "AF14", "AD13", "AC13", "C13", "B13", "A13",
      "N1", "P1", "P2", "T7", "U3", "U4", "V1", "V2"]
LEDR = ["AE23", "AF23", "AB21", "AC22", "AD22", "AD23", "AD21", "AC21", "AA14",
        "Y13", "AA13", "AC14", "AD15", "AE15", "AF13", "AE13", "AE12", "AD12"]

out = pathlib.Path(sys.argv[1])
seed = int(sys.argv[2])
chains = int(sys.argv[3]) if len(sys.argv) > 3 else 3
per = int(sys.argv[4]) if len(sys.argv) > 4 else 5
rng = random.Random(seed)

used = set()
def place():
    while True:
        loc = (rng.choice(LAB_X), rng.choice(LAB_Y), rng.randrange(0, 32, 2))
        if loc not in used:
            used.add(loc)
            return loc

v = [f"module top(input [{chains-1}:0] SW, output [{chains-1}:0] LEDR);"]
qsf = []
for c in range(chains):
    prev = f"SW[{c}]"
    for k in range(per):
        name = f"c{c}_{k}"
        nxt = f"LEDR[{c}]" if k == per - 1 else f"w{c}_{k}"
        if k != per - 1:
            # keep: otherwise quartus_map folds the buffer chain down to two LUTs
            v.append(f"  (* keep *) wire {nxt};")
        v.append(f"  cycloneii_lcell_comb #(.lut_mask(16'hAAAA), .sum_lutc_input(\"datac\")) {name} (")
        v.append(f"    .dataa({prev}), .datab(1'b1), .datac(1'b1), .datad(1'b1), .cin(1'b0), .combout({nxt}));")
        x, y, n = place()
        qsf.append(f"set_location_assignment LCCOMB_X{x}_Y{y}_N{n} -to {name}")
        prev = nxt
    qsf.append(f"set_location_assignment PIN_{SW[c]} -to SW[{c}]")
    qsf.append(f"set_location_assignment PIN_{LEDR[c]} -to LEDR[{c}]")
v.append("endmodule")

out.mkdir(parents=True, exist_ok=True)
(out / "top.v").write_text("\n".join(v) + "\n")
(out / "assignments.qsf").write_text("\n".join(qsf) + "\n")
