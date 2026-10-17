#!/usr/bin/env python3
"""Generate a logic-element/LAB feature fuzz design for tools/oracle.sh.

    gen_lab.py <out-dir> <seed> [<labs>]

A handful of random LABs, each filled with WYSIWYG cycloneii_lcell_comb +
cycloneii_lcell_ff pairs pinned to explicit LEs. Every LAB draws its own
control set (clock, inverted clock, second clock, clock enable, async
clear, sync clear, sync load) and every LE its own features (LUT using all
four inputs, arithmetic carry chain, register packed with the LUT,
register-only). The generator's intent doesn't have to survive the
fitter: learning reads the post-fit netlist (.vo), the back-annotated
placement and the .rcf, never this script.

The pin set is fixed (both clocks, 18 switches, 4 keys, 18 red LEDs, all
used) so every design shares one I/O configuration.
"""
import pathlib, random, sys

LAB_X = [x for x in range(1, 65) if x not in (13, 26, 39, 52)]
LAB_Y = list(range(1, 36))
HOLE = {(x, y) for x in range(1, 7) for y in range(19, 23)}

SW = ["N25", "N26", "P25", "AE14", "AF14", "AD13", "AC13", "C13", "B13", "A13",
      "N1", "P1", "P2", "T7", "U3", "U4", "V1", "V2"]
KEY = ["G26", "N23", "P23", "W26"]
LEDR = ["AE23", "AF23", "AB21", "AC22", "AD22", "AD23", "AD21", "AC21", "AA14",
        "Y13", "AA13", "AC14", "AD15", "AE15", "AF13", "AE13", "AE12", "AD12"]

out = pathlib.Path(sys.argv[1])
seed = int(sys.argv[2])
nlabs = int(sys.argv[3]) if len(sys.argv) > 3 else 6
rng = random.Random(seed)

body, qsf = [], []
pins = [f"SW[{i}]" for i in range(18)] + [f"KEY[{i}]" for i in range(4)]
# Every pin and every cell output must be consumed, or synthesis prunes it
# and the pin set (and with it the I/O config) changes between designs.
unused = list(pins)
avail = list(pins)
ctr = 0


def fresh(p):
    global ctr
    ctr += 1
    return f"{p}{ctr}"


def pick():
    """A source signal, preferring ones nothing reads yet."""
    if unused and rng.random() < 0.6:
        return unused.pop(rng.randrange(len(unused)))
    s = rng.choice(avail)
    if s in unused:
        unused.remove(s)
    return s


def produce(s):
    avail.append(s)
    unused.append(s)


def comb(x, y, le, mask, ins, cin, lutc, cout=None):
    name, o = fresh("l"), fresh("c")
    body.append(f"  wire {o};" + (f" wire {cout};" if cout else ""))
    body.append(f"  cycloneii_lcell_comb #(.lut_mask(16'h{mask:04X}), .sum_lutc_input(\"{lutc}\")) {name} (")
    co = f", .cout({cout})" if cout else ""
    body.append(f"    .dataa({ins[0]}), .datab({ins[1]}), .datac({ins[2]}), .datad({ins[3]}), .cin({cin}), .combout({o}){co});")
    qsf.append(f"set_location_assignment LCCOMB_X{x}_Y{y}_N{2 * le} -to {name}")
    return o


labs = set()
while len(labs) < nlabs:
    loc = (rng.choice(LAB_X), rng.choice(LAB_Y))
    if loc not in HOLE:
        labs.add(loc)

for (x, y) in sorted(labs):
    # LAB-wide controls: the menu every register in this LAB draws from.
    clks = [rng.choice(["CLOCK_50", "CLOCK_50", "CLOCK_27", "!CLOCK_50"])]
    if rng.random() < 0.3:
        clks.append("CLOCK_27" if "CLOCK_27" not in clks[0] else "CLOCK_50")
    enas = [pick() if rng.random() < 0.5 else "1'b1" for _ in clks]
    aclr = pick() if rng.random() < 0.5 else None
    sclr = pick() if rng.random() < 0.4 else None
    sload = pick() if rng.random() < 0.4 else None
    # A LAB has only 4 local (non-global) control lines (asmdb
    # LAB_PIA_TO_INPUT); drop extras rather than make the fitter give up.
    while len({s for s in enas + [aclr, sclr, sload] if s and s != "1'b1"}) > 4:
        sload = None if sload else sload
        if sload is None and len({s for s in enas + [aclr, sclr] if s and s != "1'b1"}) > 4:
            sclr = None

    def ff(le, d, datac=None):
        q, name = fresh("q"), fresh("f")
        k = rng.randrange(len(clks))
        ports = [f".datain({d})", f".clk({clks[k]})", f".ena({enas[k]})"]
        if aclr and rng.random() < 0.7:
            ports.append(f".aclr({aclr})")
        if sclr and rng.random() < 0.7:
            ports.append(f".sclr({sclr})")
        if sload and rng.random() < 0.7:
            # sdata enters through the LE's datac, so it must be the LUT's
            # own datac signal or the LE runs out of inputs.
            ports += [f".sload({sload})", f".sdata({datac or pick()})"]
        body.append(f"  wire {q};")
        body.append(f"  cycloneii_lcell_ff {name} ({', '.join(ports)}, .regout({q}));")
        qsf.append(f"set_location_assignment LCFF_X{x}_Y{y}_N{2 * le + 1} -to {name}")
        produce(q)

    le = 0
    while le < 16:
        kind = rng.choices(["lut", "lutff", "ffonly", "arith", "skip"], [3, 4, 2, 2, 1])[0]
        if kind == "skip":
            le += 1
        elif kind == "arith":
            n = min(rng.randint(2, 6), 16 - le)
            cin = rng.choice(["1'b0", "1'b1"])
            for k in range(n):
                cout = fresh("co")
                # Upper half: sum = a ^ b ^ cin (sum_lutc_input = cin);
                # lower half: carry = majority. Sometimes perturb the sum.
                mask = 0x96E8 ^ (rng.randrange(1, 16) << 12 if rng.random() < 0.3 else 0)
                s = comb(x, y, le, mask, [pick(), pick(), "1'b1", "1'b1"], cin, "cin", cout)
                if rng.random() < 0.7:
                    ff(le, s)
                else:
                    produce(s)
                cin = cout
                le += 1
        else:
            o, ins = None, None
            if kind != "ffonly":
                ins = [pick() for _ in range(4)]
                o = comb(x, y, le, rng.randrange(1, 0xFFFF), ins, "1'b0", "datac")
            if kind != "lut":
                ff(le, o or pick(), ins[2] if ins else None)
                if o and rng.random() < 0.5:
                    produce(o)
            else:
                produce(o)
            le += 1

# Fold whatever nothing reads into the 18 LEDs with free-placed XOR LUTs.
leds = [[] for _ in range(18)]
for i, s in enumerate(unused):
    leds[i % 18].append(s)
for i, srcs in enumerate(leds):
    while len(srcs) > 4:
        a = srcs[:4]
        srcs = srcs[4:]
        n, o = fresh("x"), fresh("xo")
        body.append(f"  wire {o};")
        body.append(f"  cycloneii_lcell_comb #(.lut_mask(16'h6996), .sum_lutc_input(\"datac\")) {n} (.dataa({a[0]}), .datab({a[1]}), .datac({a[2]}), .datad({a[3]}), .cin(1'b0), .combout({o}));")
        srcs.append(o)
    srcs = (srcs + [rng.choice(avail)] * 4)[:4] if srcs else [rng.choice(avail)] * 4
    body.append(f"  cycloneii_lcell_comb #(.lut_mask(16'h6996), .sum_lutc_input(\"datac\")) led{i} (.dataa({srcs[0]}), .datab({srcs[1]}), .datac({srcs[2]}), .datad({srcs[3]}), .cin(1'b0), .combout(LEDR[{i}]));")

v = ["module top(input CLOCK_50, input CLOCK_27, input [17:0] SW, input [3:0] KEY, output [17:0] LEDR);"]
v += body + ["endmodule"]
qsf.append("set_location_assignment PIN_N2 -to CLOCK_50")
qsf.append("set_location_assignment PIN_D13 -to CLOCK_27")
qsf += [f"set_location_assignment PIN_{p} -to SW[{i}]" for i, p in enumerate(SW)]
qsf += [f"set_location_assignment PIN_{p} -to KEY[{i}]" for i, p in enumerate(KEY)]
qsf += [f"set_location_assignment PIN_{p} -to LEDR[{i}]" for i, p in enumerate(LEDR)]
out.mkdir(parents=True, exist_ok=True)
(out / "top.v").write_text("\n".join(v) + "\n")
(out / "assignments.qsf").write_text("\n".join(qsf) + "\n")
