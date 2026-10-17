#!/usr/bin/env python3
"""Generate a one-LUT test design for tools/oracle.sh.

    gen_lut.py <out-dir> <lut_mask hex> [<LCCOMB_X.._Y.._N..>] [<routing.rcf>]

The LUT is a WYSIWYG cycloneii_lcell_comb so synthesis can't rewrite the
truth table. SW[3:0] -> dataa..datad, combout -> LEDR0. Passing an .rcf pins
the routing, including which physical LUT input each signal lands on (the
fitter otherwise permutes inputs and rewrites the mask to match).
"""
import pathlib, shutil, sys

out = pathlib.Path(sys.argv[1])
mask = int(sys.argv[2], 16)
loc = sys.argv[3] if len(sys.argv) > 3 and sys.argv[3] != "-" else None
rcf = sys.argv[4] if len(sys.argv) > 4 else None

out.mkdir(parents=True, exist_ok=True)
(out / "top.v").write_text(f"""module top(input [3:0] SW, output LEDR0);
  cycloneii_lcell_comb #(.lut_mask(16'h{mask:04X}), .sum_lutc_input("datac")) lut (
    .dataa(SW[0]), .datab(SW[1]), .datac(SW[2]), .datad(SW[3]), .cin(1'b0), .combout(LEDR0));
endmodule
""")
qsf = [
    "set_location_assignment PIN_AE23 -to LEDR0",
    "set_location_assignment PIN_N25 -to SW[0]",
    "set_location_assignment PIN_N26 -to SW[1]",
    "set_location_assignment PIN_P25 -to SW[2]",
    "set_location_assignment PIN_AE14 -to SW[3]",
]
if loc:
    qsf.append(f"set_location_assignment {loc} -to lut")
if rcf:
    shutil.copy(rcf, out / "routing.rcf")
    qsf += [
        "set_global_assignment -name ROUTING_BACK_ANNOTATION_MODE NORMAL",
        "set_global_assignment -name ROUTING_BACK_ANNOTATION_FILE ../routing.rcf",
    ]
(out / "assignments.qsf").write_text("\n".join(qsf) + "\n")
