#!/usr/bin/env python3
"""Generate a free-running counter design (the "blinky" every board starts
with) for tools/oracle.sh: the validation target for LE, carry chain,
register and global clock modelling.

    gen_counter.py <out-dir> <width> [--clk N2|D13] [--at X,Y] [--en]

LEDR[17:0] shows the top 18 bits. --clk picks the clock pin (N2 = DE2
CLOCK_50, D13 = CLOCK_27). --at pins counter bit k's register to LAB
(X, Y + k // 16), LE k % 16, so carry chains cross LAB boundaries at a
known place. --en adds SW[0] as clock enable and KEY[0] as active-low
synchronous clear, which pulls in LAB-wide control signals.
"""
import argparse, pathlib

LEDR = ["AE23", "AF23", "AB21", "AC22", "AD22", "AD23", "AD21", "AC21", "AA14",
        "Y13", "AA13", "AC14", "AD15", "AE15", "AF13", "AE13", "AE12", "AD12"]
CLK = {"N2": "CLOCK_50", "D13": "CLOCK_27"}

ap = argparse.ArgumentParser()
ap.add_argument("out")
ap.add_argument("width", type=int)
ap.add_argument("--clk", default="N2", choices=CLK)
ap.add_argument("--at")
ap.add_argument("--en", action="store_true")
a = ap.parse_args()
assert a.width >= 18
clk = CLK[a.clk]
ports = [f"input {clk}"] + (["input [0:0] SW", "input [0:0] KEY"] if a.en else []) + ["output [17:0] LEDR"]
step = "if (!KEY[0]) cnt <= 0; else if (SW[0]) cnt <= cnt + 1'b1;" if a.en else "cnt <= cnt + 1'b1;"
v = f"""module top({', '.join(ports)});
  reg [{a.width - 1}:0] cnt;
  always @(posedge {clk}) {step}
  assign LEDR = cnt[{a.width - 1}:{a.width - 18}];
endmodule
"""
qsf = [f"set_location_assignment PIN_{a.clk} -to {clk}"]
qsf += [f"set_location_assignment PIN_{p} -to LEDR[{i}]" for i, p in enumerate(LEDR)]
if a.en:
    qsf += ["set_location_assignment PIN_N25 -to SW[0]", "set_location_assignment PIN_G26 -to KEY[0]"]
if a.at:
    x, y = map(int, a.at.split(","))
    qsf += [f"set_location_assignment LCFF_X{x}_Y{y + k // 16}_N{2 * (k % 16) + 1} -to cnt[{k}]" for k in range(a.width)]
out = pathlib.Path(a.out)
out.mkdir(parents=True, exist_ok=True)
(out / "top.v").write_text(v)
(out / "assignments.qsf").write_text("\n".join(qsf) + "\n")
