#!/usr/bin/env python3
"""Generate an asynchronous-reset design for tools/oracle.sh: counters and
plain registers in core LEs with clears and presets of either polarity, the
`always @(posedge clk or negedge rst_n)` of Terasic and university DE2 code
among them, optionally pinned into one LAB to see what a LAB's two clear
lines can carry.

    gen_arst.py <out-dir> <json config>

Config: {"groups": [group, ...], "clk": "CLOCK_50", "global": {"s1": false}}.
A group is a set of registers sharing one reset:
  kind   "cnt": a counter (its top bit to a pin), "reg": plain registers
         (each from its own input pin to its own output pin)
  w      width
  clr    asynchronous clear: "s1" (active high, `posedge s1`), "~s1" (active
         low, `negedge s1` / `if (!s1)`), or logic ("s1&s4", "~(s1&s4)")
  pre    asynchronous preset, same forms
  ce     clock enable (same forms, `if (ce)`)
  at     [x, y, le]: register k goes to LCFF X x, Y y, LE le + k
global: per control switch, GLOBAL_SIGNAL ON / OFF (default: the fitter's
choice).
"""
import json, pathlib, re, sys

CLK = {"CLOCK_50": "N2", "CLOCK_27": "D13"}
# SW[0..7] (s3-s6 are the bottom edge's dedicated clock pins), then KEY[0..3].
CTRL = ["N25", "N26", "P25", "AE14", "AF14", "AD13", "AC13", "C13", "G26", "N23", "P23", "W26"]
# GPIO_1 then GPIO_0.
POOL = ["K25", "K26", "M22", "M23", "M19", "M20", "N20", "M21", "M24", "M25",
        "N24", "P24", "R25", "R24", "R20", "T22", "T23", "T24", "T25", "T18",
        "T21", "T20", "U26", "U25", "U23", "U24", "R19", "T19", "U20", "U21",
        "V26", "V25", "V24", "V23", "W25", "W23",
        "D25", "J22", "E26", "E25", "F24", "F23", "J21", "J20", "F25", "F26",
        "N18", "P18", "G23", "G24", "K22", "G25", "H23", "H24", "J23", "J24",
        "H25", "H26", "H19", "K18", "K19", "K21", "K23", "K24", "L21", "L20",
        "J25", "J26", "L23", "L24", "L25", "L19"]

out = pathlib.Path(sys.argv[1])
cfg = json.loads(sys.argv[2])
clk = cfg.get("clk", "CLOCK_50")
ports, body, qsf = ["input clk"], [], [f"set_location_assignment PIN_{CLK[clk]} -to clk"]
used_ctrl = set()
pool = iter(POOL)


def ctrl(e):
    """Control expression -> Verilog, noting the switches it reads."""
    for s in re.findall(r"s(\d+)", e):
        used_ctrl.add(int(s))
    return e


def fresh(direction, name):
    pin = next(pool)
    ports.append(f"{direction} {name}")
    qsf.append(f"set_location_assignment PIN_{pin} -to {name}")
    return name


def asy(spec, key, name, value):
    """Sensitivity entry and the `if` of one asynchronous control. A bare
    active-low switch is written as Terasic writes it (`negedge rst_n`,
    `if (!rst_n)`); logic goes through a wire."""
    e = spec[key]
    m = re.fullmatch(r"~(s\d+)", e)
    if m:
        ctrl(e)
        return f"negedge {m[1]}", f"if (!{m[1]}) {name} <= {value};"
    if re.fullmatch(r"s\d+", e):
        ctrl(e)
        return f"posedge {e}", f"if ({e}) {name} <= {value};"
    w = f"{name}_{key}"
    body.append(f"  wire {w} = {ctrl(e)};")
    return f"posedge {w}", f"if ({w}) {name} <= {value};"


for g, grp in enumerate(cfg["groups"]):
    w = grp["w"]
    name = f"r{g}"
    body.append(f"  reg [{w - 1}:0] {name};")
    sens, stmt = ["posedge clk"], []
    for key, value in (("clr", f"{w}'d0"), ("pre", f"{{{w}{{1'b1}}}}")):
        if key in grp:
            s, i = asy(grp, key, name, value)
            sens.append(s)
            stmt.append(i)
    if grp["kind"] == "cnt":
        last = f"{name} <= {name} + 1'b1;"
        body.append(f"  assign {fresh('output', f'o{g}')} = {name}[{w - 1}];")
    else:
        d = f"d{g}"
        ports.append(f"input [{w - 1}:0] {d}")
        qsf.extend(f"set_location_assignment PIN_{next(pool)} -to {d}[{k}]" for k in range(w))
        ports.append(f"output [{w - 1}:0] o{g}")
        qsf.extend(f"set_location_assignment PIN_{next(pool)} -to o{g}[{k}]" for k in range(w))
        body.append(f"  assign o{g} = {name};")
        last = f"{name} <= {d};"
    if "ce" in grp:
        last = f"if ({ctrl(grp['ce'])}) {last}"
    stmt.append(last)
    body.append(f"  always @({' or '.join(sens)}) " + " else ".join(stmt))
    if "at" in grp:
        x, y, le = grp["at"]
        qsf.extend(f"set_location_assignment LCFF_X{x}_Y{y}_N{2 * (le + k) + 1} -to {name}[{k}]" for k in range(w))

for c in sorted(used_ctrl):
    ports.append(f"input s{c}")
    qsf.append(f"set_location_assignment PIN_{CTRL[c]} -to s{c}")
for s, on in cfg.get("global", {}).items():
    qsf.append(f"set_instance_assignment -name GLOBAL_SIGNAL {'ON' if on else 'OFF'} -to {s}")

out.mkdir(parents=True, exist_ok=True)
(out / "top.v").write_text("module top(" + ", ".join(ports) + ");\n" + "\n".join(body) + "\nendmodule\n")
(out / "assignments.qsf").write_text("\n".join(qsf) + "\n")
(out / "config.json").write_text(json.dumps(cfg) + "\n")
