#!/usr/bin/env python3
"""Generate an I/O-cell register control design for tools/oracle.sh: input,
output and OE registers asked into their I/O cells (FAST_*_REGISTER) with
clock enables, clears, presets, synchronous clears and sets, and falling-edge
clocks.

    gen_ioce.py <out-dir> <json config>

Config: {"slots": [slot, ...], "clk": "CLOCK_50"}. Each slot is one pin:
  pin    package pin, e.g. "V6"
  in     input register spec (the pin is read through a register)
  out    output register spec (the pin's data comes from a register)
  oe     OE register spec (the pin's enable comes from a register)
  A pin with in and out/oe is an inout; with oe and no out its data is a
  control pin straight. A register spec is an object:
    ce    clock enable: a control name ("s0".."s7": a switch pin) or
          "~s0" (active low) or "s0&s1" (logic)
    clr   asynchronous clear (same forms)
    pre   asynchronous preset
    sclr  synchronous clear
    sset  synchronous set
    neg   true: falling-edge clock
    init  1: power-up value 1
    fast  false: no FAST_*_REGISTER assignment (see whether Quartus packs it)
Each input register's output goes to its own output pin (GPIO_1, plain),
each output/OE register's data from its own input pin (GPIO_1).
"""
import json, pathlib, sys

CLK = {"CLOCK_50": "N2", "CLOCK_27": "D13"}
# SW[0..7]: the controls.
CTRL = ["N25", "N26", "P25", "AE14", "AF14", "AD13", "AC13", "C13"]
# GPIO_1: register data in and input registers' values out.
POOL = ["K25", "K26", "M22", "M23", "M19", "M20", "N20", "M21", "M24", "M25",
        "N24", "P24", "R25", "R24", "R20", "T22", "T23", "T24", "T25", "T18",
        "T21", "T20", "U26", "U25", "U23", "U24", "R19", "T19", "U20", "U21",
        "V26", "V25", "V24", "V23", "W25", "W23"]

out = pathlib.Path(sys.argv[1])
cfg = json.loads(sys.argv[2])
clk = cfg.get("clk", "CLOCK_50")
ports, body, qsf = [f"input clk"], [], [f"set_location_assignment PIN_{CLK[clk]} -to clk"]
used_ctrl = set()
pool = iter(POOL)


def ctrl(e):
    """Control expression -> Verilog, noting the switches it reads."""
    import re
    for s in re.findall(r"s(\d)", e):
        used_ctrl.add(int(s))
    return e


def fresh(direction, name):
    pin = next(pool)
    ports.append(f"{direction} {name}")
    qsf.append(f"set_location_assignment PIN_{pin} -to {name}")
    return name


def register(name, d, spec):
    """`reg name` sampling d under spec's controls."""
    edge = "negedge" if spec.get("neg") else "posedge"
    sens = [f"{edge} clk"]
    asy = []
    if "clr" in spec:
        sens.append(f"posedge {name}_clr")
        body.append(f"  wire {name}_clr = {ctrl(spec['clr'])};")
        asy.append((f"{name}_clr", "1'b0"))
    if "pre" in spec:
        sens.append(f"posedge {name}_pre")
        body.append(f"  wire {name}_pre = {ctrl(spec['pre'])};")
        asy.append((f"{name}_pre", "1'b1"))
    init = " = 1'b1" if spec.get("init") == 1 else ""
    body.append(f"  reg {name}{init};")
    stmt = []
    for c, v in asy:
        stmt.append(f"if ({c}) {name} <= {v};")
    if "sclr" in spec:
        stmt.append(f"if ({ctrl(spec['sclr'])}) {name} <= 1'b0;")
    if "sset" in spec:
        stmt.append(f"if ({ctrl(spec['sset'])}) {name} <= 1'b1;")
    last = f"{name} <= {d};"
    if "ce" in spec:
        last = f"if ({ctrl(spec['ce'])}) {last}"
    stmt.append(last)
    body.append(f"  always @({' or '.join(sens)}) " + " else ".join(stmt))


for k, s in enumerate(cfg["slots"]):
    p = f"p{k}"
    has_in, has_out, has_oe = "in" in s, "out" in s, "oe" in s
    direction = "inout" if has_in and (has_out or has_oe) else "input" if has_in else "output"
    ports.append(f"{direction} {p}")
    qsf.append(f"set_location_assignment PIN_{s['pin']} -to {p}")
    if has_in:
        register(f"q{k}", p, s["in"])
        o = fresh("output", f"o{k}")
        body.append(f"  assign {o} = q{k};")
        if s["in"].get("fast", True):
            qsf.append(f"set_instance_assignment -name FAST_INPUT_REGISTER ON -to {p}")
    data = None
    if has_out:
        register(f"d{k}", fresh("input", f"di{k}"), s["out"])
        data = f"d{k}"
        if s["out"].get("fast", True):
            qsf.append(f"set_instance_assignment -name FAST_OUTPUT_REGISTER ON -to {p}")
    if has_oe:
        register(f"e{k}", fresh("input", f"ei{k}"), s["oe"])
        data = data or fresh("input", f"dd{k}")
        body.append(f"  assign {p} = e{k} ? {data} : 1'bz;")
        if s["oe"].get("fast", True):
            qsf.append(f"set_instance_assignment -name FAST_OUTPUT_ENABLE_REGISTER ON -to {p}")
    elif has_out:
        body.append(f"  assign {p} = {data};")

for c in sorted(used_ctrl):
    ports.append(f"input s{c}")
    qsf.append(f"set_location_assignment PIN_{CTRL[c]} -to s{c}")

out.mkdir(parents=True, exist_ok=True)
(out / "top.v").write_text("module top(" + ", ".join(ports) + ");\n" + "\n".join(body) + "\nendmodule\n")
(out / "assignments.qsf").write_text("\n".join(qsf) + "\n")
(out / "config.json").write_text(json.dumps(cfg) + "\n")
