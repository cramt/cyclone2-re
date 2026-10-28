#!/usr/bin/env python3
"""Generate a multiplier fuzz design: WYSIWYG cycloneii_mac_mult +
cycloneii_mac_out atoms fed from a counter xor the DE2's switches, products
folded onto the LEDs, so nothing is optimised away and few pins are needed.

    gen_mult.py <out-dir> <json config>

Config: {"mults": [<mult>, ...], "seed": n (only for "random" fields)}.
One <mult> is one mac_mult -> mac_out pair; keys (all optional):
  wa, wb        operand widths (1-18; default 18)
  sa, sb        sign of operand a / b: "0" unsigned, "1" signed, "dyn" a
                switch (SW[17] / SW[16]) decides
  areg, breg    input register on dataa / datab
  sareg, sbreg  input register on signa / signb
  oreg          output register (mac_out)
  ena           clock enable on every register of the pair (KEY[3])
  aclr          false | "key" (KEY[1]) | "logic" (SW[15] & SW[14])
  clk           "50" (CLOCK_50, default) | "27" (CLOCK_27)
  inv           invert the data operands in front of the block
  loc           location of the mac_mult atom (e.g. "MAC_MULT_X39_Y5_N0");
                oloc that of the mac_out
  base          counter/switch offset of this pair's operands
"""
import json, pathlib, sys

out = pathlib.Path(sys.argv[1])
cfg = json.loads(sys.argv[2]) if len(sys.argv) > 2 else {}
mults = cfg.get("mults", [{}])

total_in = sum(m.get("wa", 18) + m.get("wb", 18) for m in mults)
cnt_w = 40
v = ["module top(input CLOCK_50, input CLOCK_27, input [17:0] SW, input [3:0] KEY, output [17:0] LEDR, output [8:0] LEDG);",
     f"  reg [{cnt_w - 1}:0] cnt;",
     "  always @(posedge CLOCK_50) cnt <= cnt + 1'b1;",
     "  wire lclr = SW[15] & SW[14];"]


def bus(w, base, inv):
    parts = []
    for i in range(w):
        e = f"cnt[{(base + 3 * i) % cnt_w}] ^ SW[{(base + i) % 14}]"
        parts.append(f"~({e})" if inv else e)
    return "{" + ", ".join(reversed(parts)) + "}"


def sign(s, sw):
    return {"0": "1'b0", "1": "1'b1", "dyn": f"SW[{sw}]"}[s]


qsf_extra = []
outs = []
for k, m in enumerate(mults):
    wa, wb = m.get("wa", 18), m.get("wb", 18)
    base = m.get("base", 7 * k)
    inv = m.get("inv", False)
    clk = {"50": "CLOCK_50", "27": "CLOCK_27"}[m.get("clk", "50")]
    aclr = {False: "1'b0", "key": "KEY[1]", "logic": "lclr"}[m.get("aclr", False)]
    ena = "KEY[3]" if m.get("ena", False) else "1'b1"
    regs = {r: m.get(r, False) for r in ("areg", "breg", "sareg", "sbreg")}
    oreg = m.get("oreg", False)
    w = wa + wb
    v.append(f"  wire [{w - 1}:0] p{k}, q{k};")
    mp = {
        "dataa_width": wa, "datab_width": wb,
        "dataa_clock": '"0"' if regs["areg"] else '"none"',
        "datab_clock": '"0"' if regs["breg"] else '"none"',
        "signa_clock": '"0"' if regs["sareg"] else '"none"',
        "signb_clock": '"0"' if regs["sbreg"] else '"none"',
    }
    conns = {"dataa": bus(wa, base, inv), "datab": bus(wb, base + 5, inv),
             "signa": sign(m.get("sa", "0"), 17), "signb": sign(m.get("sb", "0"), 16),
             "dataout": f"p{k}"}
    if any(regs.values()):
        conns.update({"clk": clk, "aclr": aclr, "ena": ena})
    v.append(f"  cycloneii_mac_mult m{k} (" + ", ".join(f".{a}({b})" for a, b in conns.items()) + ");")
    v += [f"  defparam m{k}.{a} = {b};" for a, b in mp.items()]
    oconns = {"dataa": f"p{k}", "dataout": f"q{k}"}
    if oreg:
        oconns.update({"clk": clk, "aclr": aclr, "ena": ena})
    v.append(f"  cycloneii_mac_out o{k} (" + ", ".join(f".{a}({b})" for a, b in oconns.items()) + ");")
    v += [f"  defparam o{k}.dataa_width = {w};", f'  defparam o{k}.output_clock = "{"0" if oreg else "none"}";']
    if "loc" in m:
        qsf_extra.append(f"set_location_assignment {m['loc']} -to m{k}")
    if "oloc" in m:
        qsf_extra.append(f"set_location_assignment {m['oloc']} -to o{k}")
    outs += [f"q{k}[{i}]" for i in range(w)]

groups = [[] for _ in range(27)]
for i, o in enumerate(outs):
    groups[i % 27].append(o)
for i, g in enumerate(groups):
    rhs = " ^ ".join(g) if g else "1'b0"
    tgt = f"LEDR[{i}]" if i < 18 else f"LEDG[{i - 18}]"
    v.append(f"  assign {tgt} = {rhs};")
v.append("endmodule")
out.mkdir(parents=True, exist_ok=True)
(out / "top.v").write_text("\n".join(v) + "\n")
qsf = [l for l in (pathlib.Path(__file__).parent.parent / "examples/de2.qsf").read_text().splitlines()
       if l.startswith("set_location_assignment")]
(out / "assignments.qsf").write_text("\n".join(qsf + qsf_extra) + "\n")
(out / "config.json").write_text(json.dumps(cfg) + "\n")
