#!/usr/bin/env python3
"""Generate an output-enable / bidirectional pin fuzz design for tools/oracle.sh.

    gen_oe.py <out-dir> <seed> [fraction] [variant]

Every user I/O pin is independently used with probability `fraction`
(default 1/4). A used pin is an input (a source for the others), a plain
output reading a bidir's input side, or one of the tri-state shapes in
SHAPES, picked at random. Sources are random input pins, so the core
holds routing plus the odd LUT (OE from logic) or register (OE from a
fabric register). A dedicated clock pin clocks every register.

Variants (default `base`; adding one leaves the others' seeds as they were):
  base   SHAPES
  reg    SHAPES plus REG_SHAPES (more I/O-register combinations)
  inv    reg plus INV_SHAPES (I/O registers that power up high)
  attr   SHAPES plus random drive strength, weak pull-up or bus hold
  std    SHAPES plus one random I/O standard per bank (gen_io.DRIVES)

The pin list comes from the empty design's fitter report, as in gen_io.py.
"""
import pathlib, random, sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from gen_io import DRIVES, pins  # noqa: E402

# shape -> port direction
SHAPES = {
    "tri": "output",        # oe from a pin, data from a pin
    "tri_inv": "output",    # oe inverted
    "tri_and": "output",    # oe from a LUT (two pins ANDed)
    "tri_ff": "output",     # oe from a fabric register (LUT + FF)
    "tri_dinv": "output",   # data inverted
    "od_out": "output",     # open drain: d ? z : 0
    "bidir": "inout",       # oe + data from pins, input side read
    "bidir_inv": "inout",   # oe inverted
    "bidir_and": "inout",   # oe from a LUT
    "od": "inout",          # open drain, input side read
    "oe1": "inout",         # oe constant 1, input side read
    "oe0": "inout",         # oe constant 0 (never driven), input side read
    "oereg": "inout",       # FAST_OUTPUT_ENABLE_REGISTER only
    "bothreg": "inout",     # output + OE registers
    "inreg": "inout",       # FAST_INPUT_REGISTER on the input side
}
REG_SHAPES = {
    "od_reg": "inout",      # open drain from FAST_OUTPUT_REGISTER
    "oereg_inv": "inout",   # OE register, OE inverted after it
    "allreg": "inout",      # input, output and OE registers
    "oereg_out": "output",  # OE register on a tri-state output
}
INV_SHAPES = {
    "outreg_inv": "inout",  # output register, data inverted after it
    "inreg_init1": "inout", # input register with init 1
}


def main():
    out = pathlib.Path(sys.argv[1])
    seed = int(sys.argv[2])
    frac = float(sys.argv[3]) if len(sys.argv) > 3 else 1 / 4
    variant = sys.argv[4] if len(sys.argv) > 4 else "base"
    shapes = {"reg": dict(SHAPES, **REG_SHAPES), "inv": dict(SHAPES, **REG_SHAPES, **INV_SHAPES)}.get(variant, SHAPES)
    rng = random.Random(seed)
    allp = pins()
    clk = rng.choice([p for p in allp if p[2] == "clk"])
    allp = [p for p in allp if p != clk]

    ins, driven = [], []
    for p in allp:
        if rng.random() >= frac:
            continue
        if p[2] == "clk" or rng.random() < 0.3:
            ins.append(p)
        else:
            driven.append((p, rng.choice(list(shapes))))
    if not ins:
        ins.append(driven.pop()[0])

    v, qsf, decl = [], [f"set_location_assignment PIN_{clk[0]} -to clk"], ["input clk"]
    src = lambda: f"i{rng.randrange(len(ins))}"
    for k, p in enumerate(ins):
        decl.append(f"input i{k}")
        qsf.append(f"set_location_assignment PIN_{p[0]} -to i{k}")
    readable = []  # bidir ports whose input side can be read
    for k, (p, shape) in enumerate(driven):
        n = f"p{k}"
        decl.append(f"{shapes[shape]} {n}")
        qsf.append(f"set_location_assignment PIN_{p[0]} -to {n}")
        d, e = src(), src()
        if shape in ("tri", "bidir", "inreg"):
            v.append(f"  assign {n} = {e} ? {d} : 1'bz;")
        elif shape in ("tri_inv", "bidir_inv"):
            v.append(f"  assign {n} = ~{e} ? {d} : 1'bz;")
        elif shape in ("tri_and", "bidir_and"):
            v.append(f"  assign {n} = ({e} & {src()}) ? {d} : 1'bz;")
        elif shape == "tri_ff":
            v.append(f"  reg e{k}; always @(posedge clk) e{k} <= {e} ^ {src()};")
            v.append(f"  assign {n} = e{k} ? {d} : 1'bz;")
        elif shape == "tri_dinv":
            v.append(f"  assign {n} = {e} ? ~{d} : 1'bz;")
        elif shape in ("od", "od_out"):
            v.append(f"  assign {n} = {d} ? 1'bz : 1'b0;")
        elif shape == "oe1":
            v.append(f"  assign {n} = {d};")
        elif shape == "oe0":
            v.append(f"  assign {n} = 1'bz;")
        elif shape in ("oereg", "oereg_out"):
            v.append(f"  reg e{k}; always @(posedge clk) e{k} <= {e};")
            v.append(f"  assign {n} = e{k} ? {d} : 1'bz;")
            qsf.append(f"set_instance_assignment -name FAST_OUTPUT_ENABLE_REGISTER ON -to {n}")
        elif shape == "bothreg":
            v.append(f"  reg e{k}, d{k}; always @(posedge clk) begin e{k} <= {e}; d{k} <= {d}; end")
            v.append(f"  assign {n} = e{k} ? d{k} : 1'bz;")
            qsf.append(f"set_instance_assignment -name FAST_OUTPUT_ENABLE_REGISTER ON -to {n}")
            qsf.append(f"set_instance_assignment -name FAST_OUTPUT_REGISTER ON -to {n}")
        elif shape == "od_reg":
            v.append(f"  reg d{k}; always @(posedge clk) d{k} <= {d};")
            v.append(f"  assign {n} = d{k} ? 1'bz : 1'b0;")
            qsf.append(f"set_instance_assignment -name FAST_OUTPUT_REGISTER ON -to {n}")
        elif shape == "oereg_inv":
            v.append(f"  reg e{k}; always @(posedge clk) e{k} <= {e};")
            v.append(f"  assign {n} = ~e{k} ? {d} : 1'bz;")
            qsf.append(f"set_instance_assignment -name FAST_OUTPUT_ENABLE_REGISTER ON -to {n}")
        elif shape == "allreg":
            v.append(f"  reg e{k}, d{k}; always @(posedge clk) begin e{k} <= {e}; d{k} <= {d}; end")
            v.append(f"  assign {n} = e{k} ? d{k} : 1'bz;")
            for a in ("FAST_OUTPUT_ENABLE_REGISTER", "FAST_OUTPUT_REGISTER", "FAST_INPUT_REGISTER"):
                qsf.append(f"set_instance_assignment -name {a} ON -to {n}")
        elif shape == "outreg_inv":
            v.append(f"  reg d{k}; always @(posedge clk) d{k} <= {d};")
            v.append(f"  assign {n} = {e} ? ~d{k} : 1'bz;")
            qsf.append(f"set_instance_assignment -name FAST_OUTPUT_REGISTER ON -to {n}")
        elif shape == "inreg_init1":
            v.append(f"  assign {n} = {e} ? {d} : 1'bz;")
            v.append(f"  reg q{k} = 1'b1; always @(posedge clk) q{k} <= {n};")
            qsf.append(f"set_instance_assignment -name FAST_INPUT_REGISTER ON -to {n}")
            readable.append(f"q{k}")
        if shape in ("inreg", "allreg"):
            v.append(f"  reg q{k}; always @(posedge clk) q{k} <= {n};")
            qsf.append(f"set_instance_assignment -name FAST_INPUT_REGISTER ON -to {n}")
            readable.append(f"q{k}")
        elif shapes[shape] == "inout":
            readable.append(n)
    # Each readable input side drives a plain output: the readers are
    # the plain outputs among the driven pins' would-be slots, so add them
    # as extra pins taken from the unused ones.
    used = {clk[0]} | {p[0] for p in ins} | {p[0] for p, _ in driven}
    spare = [p for p in allp if p[0] not in used and p[2] == "io"]
    rng.shuffle(spare)
    for k, r in enumerate(readable):
        if not spare:
            break
        p = spare.pop()
        decl.append(f"output r{k}")
        qsf.append(f"set_location_assignment PIN_{p[0]} -to r{k}")
        v.append(f"  assign r{k} = {r};")

    if variant in ("attr", "std"):
        bank = {p[0]: p[1] for p in allp + [clk]}
        std = {b: "3.3-V LVTTL" for b in bank.values()}
        if variant == "std":
            for b in sorted(std):
                std[b] = rng.choice(list(DRIVES))
        for line in list(qsf):
            if not line.startswith("set_location_assignment"):
                continue
            pin, port = line.split()[1][4:], line.split()[-1]
            s = std[bank[pin]]
            if s != "3.3-V LVTTL":
                qsf.append(f'set_instance_assignment -name IO_STANDARD "{s}" -to {port}')
            if variant == "attr" and port.startswith("p"):
                if rng.random() < 0.7:
                    qsf.append(f'set_instance_assignment -name CURRENT_STRENGTH_NEW "{rng.choice(DRIVES[s])}" -to {port}')
                r = rng.random()
                if r < 0.25:
                    qsf.append(f"set_instance_assignment -name WEAK_PULL_UP_RESISTOR ON -to {port}")
                elif r < 0.5:
                    qsf.append(f"set_instance_assignment -name ENABLE_BUS_HOLD_CIRCUITRY ON -to {port}")

    text = [f"module top({', '.join(decl)});"] + v + ["endmodule"]
    out.mkdir(parents=True, exist_ok=True)
    (out / "top.v").write_text("\n".join(text) + "\n")
    (out / "assignments.qsf").write_text("\n".join(qsf) + "\n")
    (out / "shapes.txt").write_text("".join(f"p{k} PIN_{p[0]} {s}\n" for k, (p, s) in enumerate(driven)))


if __name__ == "__main__":
    main()
