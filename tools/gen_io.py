#!/usr/bin/env python3
"""Generate an I/O fuzz design for tools/oracle.sh.

    gen_io.py <out-dir> <seed> <campaign> [fraction]

Every user I/O pin is independently used with probability `fraction`
(default 1/3), as an input, an output or a bidir. Outputs and bidirs are
driven straight from random input pins, so the core holds only routing (no
LEs) and every used pin's IOE config can be diffed against the empty
design. Independent random inclusion is what lets a bit be pinned on one
pin even when it isn't near that pin's tile (group testing).

Campaigns:
  roles  default I/O standard (3.3-V LVTTL) and drive, roles only
  attr   roles plus random per-pin drive strength / weak pull-up / bus hold
  std    roles plus one random I/O standard per bank (default drive)
  reg    roles plus I/O-cell registers: a dedicated clock pin clocks
         FAST_INPUT/OUTPUT/OUTPUT_ENABLE_REGISTER flops on random pins
  unused (with fraction 0) nothing but the unused-pin mode under test,
         taken from the seed: see UNUSED_MODES

The pin list is read from the empty design's fitter report ("All Package
Pins"), so no Quartus-derived table lives in the repo.
"""
import pathlib, random, re, sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
REPORT = ROOT / "specimens/route/empty/build/output_files/top.fit.rpt"

# 3.3-V LVTTL/LVCMOS current strengths Quartus accepts for Cyclone II.
DRIVES = {"3.3-V LVTTL": ["4MA", "8MA", "12MA", "16MA", "20MA", "24MA"],
          "3.3-V LVCMOS": ["4MA", "8MA", "12MA", "16MA", "20MA", "24MA"],
          "2.5 V": ["4MA", "8MA", "12MA", "16MA"],
          "1.8 V": ["2MA", "4MA", "6MA", "8MA", "10MA", "12MA"],
          "1.5 V": ["2MA", "4MA", "6MA", "8MA"]}
UNUSED_MODES = ["AS INPUT TRI-STATED", "AS OUTPUT DRIVING GROUND",
                "AS OUTPUT DRIVING AN UNSPECIFIED SIGNAL",
                "AS INPUT TRI-STATED WITH BUS-HOLD",
                "AS INPUT TRI-STATED WITH WEAK PULL-UP"]


def pins():
    """(name, bank, kind) for every pin a user design may claim. kind is
    'io' or 'clk' (dedicated clock input: shows as GND+ when unused)."""
    out, on = [], False
    for line in REPORT.read_text(encoding="latin-1").splitlines():
        if line.startswith("; All Package Pins"):
            on = True
            continue
        if on and not line.strip():
            break
        f = [c.strip() for c in line.split(";")]
        if not on or len(f) < 10 or not f[3].isdigit():
            continue
        loc, usage = f[1], f[4]
        if usage == "RESERVED_INPUT" or usage == "LEDR0":
            out.append((loc, int(f[3]), "io"))
        elif usage == "GND+":
            out.append((loc, int(f[3]), "clk"))
    return out


def main():
    out = pathlib.Path(sys.argv[1])
    seed = int(sys.argv[2])
    campaign = sys.argv[3]
    frac = float(sys.argv[4]) if len(sys.argv) > 4 else 1 / 3
    rng = random.Random(seed)
    allp = pins()
    banks = sorted({b for _, b, _ in allp})
    std = {b: "3.3-V LVTTL" for b in banks}
    if campaign == "std":
        for b in banks:
            std[b] = rng.choice(list(DRIVES))

    clk = None
    if campaign == "reg":
        clk = rng.choice([p for p in allp if p[2] == "clk"])
        allp = [p for p in allp if p != clk]
    ins, outs, bidirs = [], [], []
    for name, bank, kind in allp:
        if rng.random() >= frac:
            continue
        role = "in" if kind == "clk" else rng.choice(["in", "in", "out", "out", "bidir"])
        {"in": ins, "out": outs, "bidir": bidirs}[role].append((name, bank, kind))
    if (outs or bidirs) and not ins:
        ins.append(outs.pop() if outs else bidirs.pop())

    v, qsf, inst = [], [], []
    ports = []
    if ins:
        ports.append(f"input [{len(ins)-1}:0] i")
    if outs:
        ports.append(f"output [{len(outs)-1}:0] o")
    if bidirs:
        ports.append(f"inout [{len(bidirs)-1}:0] b")
    if clk:
        ports.append("input clk")
        qsf.append(f"set_location_assignment PIN_{clk[0]} -to clk")
    regs = []  # (port, assignment) for I/O-cell registers
    # Registered inputs: the flop sits in the input pin's IOE and feeds the
    # rest of the design as a net named ri<k>.
    rin = {k for k in range(len(ins)) if clk and rng.random() < 0.5}
        # The bidirs' input side is left dead: feeding it anywhere would
        # need LUTs whose masks the core model doesn't know.
    v.append(f"module top({', '.join(ports)});")
    for k in sorted(rin):
        v.append(f"  reg ri{k}; always @(posedge clk) ri{k} <= i[{k}];")
        regs.append((f"ri{k}", "FAST_INPUT_REGISTER"))
    src_net = lambda k: f"ri{k}" if k in rin else f"i[{k}]"
    for k, (name, bank, kind) in enumerate(ins):
        qsf.append(f"set_location_assignment PIN_{name} -to i[{k}]")
        inst.append((f"i[{k}]", "in", bank, kind))
    for k, (name, bank, kind) in enumerate(outs):
        src = rng.randrange(len(ins))
        if clk and rng.random() < 0.5:
            v.append(f"  reg ro{k}; always @(posedge clk) ro{k} <= {src_net(src)}; assign o[{k}] = ro{k};")
            regs.append((f"o[{k}]", "FAST_OUTPUT_REGISTER"))
        else:
            v.append(f"  assign o[{k}] = {src_net(src)};")
        qsf.append(f"set_location_assignment PIN_{name} -to o[{k}]")
        inst.append((f"o[{k}]", "out", bank, kind))
    for k, (name, bank, kind) in enumerate(bidirs):
        d, oe = rng.randrange(len(ins)), rng.randrange(len(ins))
        if clk and rng.random() < 0.5:
            v.append(f"  reg rd{k}, re{k}; always @(posedge clk) begin rd{k} <= {src_net(d)}; re{k} <= {src_net(oe)}; end")
            v.append(f"  assign b[{k}] = re{k} ? rd{k} : 1'bz;")
            regs.append((f"b[{k}]", "FAST_OUTPUT_REGISTER"))
            regs.append((f"b[{k}]", "FAST_OUTPUT_ENABLE_REGISTER"))
        else:
            v.append(f"  assign b[{k}] = {src_net(oe)} ? {src_net(d)} : 1'bz;")
        qsf.append(f"set_location_assignment PIN_{name} -to b[{k}]")
        inst.append((f"b[{k}]", "bidir", bank, kind))
    v.append("endmodule")

    for port, role, bank, kind in inst:
        s = std[bank]
        if s != "3.3-V LVTTL":
            qsf.append(f'set_instance_assignment -name IO_STANDARD "{s}" -to {port}')
        if campaign == "attr":
            if role != "in" and rng.random() < 0.7:
                qsf.append(f'set_instance_assignment -name CURRENT_STRENGTH_NEW "{rng.choice(DRIVES[s])}" -to {port}')
            # Dedicated clock inputs have neither pull-up nor bus hold
            # (fitter error 169057).
            r = rng.random() if kind == "io" else 1
            if r < 0.25:
                qsf.append(f"set_instance_assignment -name WEAK_PULL_UP_RESISTOR ON -to {port}")
            elif r < 0.5:
                qsf.append(f"set_instance_assignment -name ENABLE_BUS_HOLD_CIRCUITRY ON -to {port}")
    for port, a in regs:
        qsf.append(f"set_instance_assignment -name {a} ON -to {port}")
    if campaign == "unused":
        qsf.append(f'set_global_assignment -name RESERVE_ALL_UNUSED_PINS "{UNUSED_MODES[seed % len(UNUSED_MODES)]}"')
    if not inst:
        # Same design as the empty baseline, so only the unused-pin mode
        # differs from it.
        v = ["module top(output LEDR0);", "  assign LEDR0 = 1'b0;", "endmodule"]
        qsf.insert(0, "set_location_assignment PIN_AE23 -to LEDR0")

    out.mkdir(parents=True, exist_ok=True)
    (out / "top.v").write_text("\n".join(v) + "\n")
    (out / "assignments.qsf").write_text("\n".join(qsf) + "\n")


if __name__ == "__main__":
    main()
