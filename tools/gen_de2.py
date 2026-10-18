#!/usr/bin/env python3
"""Generate a design that uses the DE2's own pins: CLOCK_50 drives a
counter, SW[17:0] and KEY[3:0] mix into it, LEDR[17:0] and LEDG[8:0] show
the result. A held-out target for the I/O model on the board's real pin
set (all four sides: switches on the right/bottom, LEDs bottom, keys
right, CLOCK_50 left).

    gen_de2.py <out-dir> <seed>
"""
import pathlib, random, sys

# DE2 user manual pinout.
SW = ["N25", "N26", "P25", "AE14", "AF14", "AD13", "AC13", "C13", "B13", "A13",
      "N1", "P1", "P2", "T7", "U3", "U4", "V1", "V2"]
KEY = ["G26", "N23", "P23", "W26"]
LEDR = ["AE23", "AF23", "AB21", "AC22", "AD22", "AD23", "AD21", "AC21", "AA14",
        "Y13", "AA13", "AC14", "AD15", "AE15", "AF13", "AE13", "AE12", "AD12"]
LEDG = ["AE22", "AF22", "W19", "V18", "U18", "U17", "AA20", "Y18", "Y12"]

out = pathlib.Path(sys.argv[1])
rng = random.Random(int(sys.argv[2]))
width = rng.randrange(26, 33)
ops = [rng.choice("^&|") for _ in range(18)]
v = [f"module top(input CLOCK_50, input [17:0] SW, input [3:0] KEY, output [17:0] LEDR, output [8:0] LEDG);",
     f"  reg [{width - 1}:0] cnt;",
     "  always @(posedge CLOCK_50) cnt <= cnt + 1'b1;"]
for i, op in enumerate(ops):
    v.append(f"  assign LEDR[{i}] = cnt[{width - 18 + i}] {op} SW[{i}];")
for i in range(8):
    v.append(f"  assign LEDG[{i}] = cnt[{width - 26 + i}] & ~KEY[{i % 4}];")
v.append("  assign LEDG[8] = &KEY;")
v.append("endmodule")
qsf = ["set_location_assignment PIN_N2 -to CLOCK_50"]
qsf += [f"set_location_assignment PIN_{p} -to SW[{i}]" for i, p in enumerate(SW)]
qsf += [f"set_location_assignment PIN_{p} -to KEY[{i}]" for i, p in enumerate(KEY)]
qsf += [f"set_location_assignment PIN_{p} -to LEDR[{i}]" for i, p in enumerate(LEDR)]
qsf += [f"set_location_assignment PIN_{p} -to LEDG[{i}]" for i, p in enumerate(LEDG)]
out.mkdir(parents=True, exist_ok=True)
(out / "top.v").write_text("\n".join(v) + "\n")
(out / "assignments.qsf").write_text("\n".join(qsf) + "\n")
