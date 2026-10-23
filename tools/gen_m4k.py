#!/usr/bin/env python3
"""Generate an M4K fuzz design: one altsyncram fed and observed through a
counter, the DE2's switches and LEDs, so the RAM can't be optimised away
and the design needs few pins.

    gen_m4k.py <out-dir> <json config>

Config keys (all optional):
  mode     "single" | "dual" (simple dual port) | "bidir" (true dual) | "rom"
  wa, wb   port A / B data width (1, 2, 4, 8, 9, 16, 18, 32, 36)
  outreg_a, outreg_b   output register: "none" | "clock0" | "clock1"
  clken    clock enables on the RAM's clocks (KEY[3] / KEY[2])
  rdw      mixed-port read-during-write: "DONT_CARE" | "OLD_DATA"
  clk1     port B (or the read side) on its own clock (CLOCK_27)
  aclr     output register clear: true = KEY[1] (which Quartus promotes to a
           global through the clock delay control), "logic" = SW[16] & SW[15]
  deep     n: n times the depth of one block (address decoding over blocks)
  byteena  byte enables on port A (widths 16/18/32/36)
  init     content: {"seed": n} random, {"bits": [[addr, bit], ...]} those
           bits set, {"word": k} each word = its address xor k; absent = no
           init file (all zeros)
  loc      "M4K_X26_Y20" location assignment, or absent
"""
import json, pathlib, random, sys

DEPTH = {1: 4096, 2: 2048, 4: 1024, 8: 512, 16: 256, 32: 128, 9: 512, 18: 256, 36: 128}

out = pathlib.Path(sys.argv[1])
cfg = json.loads(sys.argv[2]) if len(sys.argv) > 2 else {}
mode = cfg.get("mode", "single")
wa = cfg.get("wa", 8)
wb = cfg.get("wb", wa)
da, db_ = DEPTH[wa] * cfg.get("deep", 1), DEPTH[wb] * cfg.get("deep", 1)
aw_a, aw_b = da.bit_length() - 1, db_.bit_length() - 1


def init_words():
    i = cfg.get("init")
    if i is None:
        return None
    words = [0] * da
    if "seed" in i:
        rng = random.Random(i["seed"])
        words = [rng.getrandbits(wa) for _ in range(da)]
    elif "bits" in i:
        for a, b in i["bits"]:
            words[a] |= 1 << b
    elif "word" in i:
        words = [(a ^ i["word"]) & ((1 << wa) - 1) for a in range(da)]
    elif "linear" in i:
        # Linear bit index n = addr * wa + bit; bit set iff n has bit k set.
        k = i["linear"]
        words = [sum(1 << b for b in range(wa) if (a * wa + b) >> k & 1) for a in range(da)]
    return words


def clkname(c):
    return {"none": "UNREGISTERED", "clock0": "CLOCK0", "clock1": "CLOCK1"}[c]


words = init_words()
out.mkdir(parents=True, exist_ok=True)
if words is not None:
    mif = [f"WIDTH={wa};", f"DEPTH={da};", "ADDRESS_RADIX=HEX;", "DATA_RADIX=HEX;", "CONTENT BEGIN"]
    mif += [f"  {a:x} : {w:x};" for a, w in enumerate(words)]
    mif += ["END;"]
    (out / "init.mif").write_text("\n".join(mif) + "\n")

wmax = max(wa, wb)
cnt_w = max(aw_a, aw_b) + wmax + 4
clk1 = cfg.get("clk1", False)
clken = cfg.get("clken", False)
aclr = cfg.get("aclr", False)
byteena = cfg.get("byteena", False) and wa >= 16
nbe = wa // (9 if wa % 9 == 0 else 8) if byteena else 1
v = ["module top(input CLOCK_50, input CLOCK_27, input [17:0] SW, input [3:0] KEY, output [17:0] LEDR, output [8:0] LEDG);",
     f"  reg [{cnt_w - 1}:0] cnt;",
     "  always @(posedge CLOCK_50) cnt <= cnt + 1'b1;",
     f"  wire [{wa - 1}:0] qa;",
     f"  wire [{wb - 1}:0] qb;",
     "  wire clr = SW[16] & SW[15];"]


def bus(w, base):
    """w bits of counter xor switches, starting at counter bit base."""
    parts = []
    for i in range(w):
        parts.append(f"cnt[{(base + i) % cnt_w}] ^ SW[{(base + i) % 18}]")
    return "{" + ", ".join(reversed(parts)) + "}"


p = {
    "intended_device_family": '"Cyclone II"',
    "lpm_type": '"altsyncram"',
    "ram_block_type": '"M4K"',
    "width_a": wa, "widthad_a": aw_a, "numwords_a": da,
    "outdata_reg_a": f'"{clkname(cfg.get("outreg_a", "none"))}"',
    "power_up_uninitialized": '"FALSE"',
}
if words is not None:
    p["init_file"] = '"../init.mif"'
conns = {"clock0": "CLOCK_50", "address_a": bus(aw_a, 0)}
if clk1:
    conns["clock1"] = "CLOCK_27"
if clken:
    conns["clocken0"] = "KEY[3]"
    if clk1:
        conns["clocken1"] = "KEY[2]"
if mode == "rom":
    p["operation_mode"] = '"ROM"'
    conns["q_a"] = "qa"
elif mode == "single":
    p["operation_mode"] = '"SINGLE_PORT"'
    p["read_during_write_mode_port_a"] = '"NEW_DATA_NO_NBE_READ"'
    conns.update({"wren_a": "KEY[0]", "data_a": bus(wa, 3), "q_a": "qa"})
else:
    p["operation_mode"] = '"DUAL_PORT"' if mode == "dual" else '"BIDIR_DUAL_PORT"'
    p.update({"width_b": wb, "widthad_b": aw_b, "numwords_b": db_})
    rclk = "CLOCK1" if clk1 else "CLOCK0"
    p["address_reg_b"] = f'"{rclk}"'
    p["outdata_reg_b"] = f'"{clkname(cfg.get("outreg_b", "none"))}"'
    p["read_during_write_mode_mixed_ports"] = f'"{cfg.get("rdw", "DONT_CARE")}"'
    conns.update({"wren_a": "KEY[0]", "data_a": bus(wa, 3), "address_b": bus(aw_b, 5), "q_b": "qb"})
    if mode == "bidir":
        p["indata_reg_b"] = f'"{rclk}"'
        p["wrcontrol_wraddress_reg_b"] = f'"{rclk}"'
        p["read_during_write_mode_port_a"] = '"NEW_DATA_NO_NBE_READ"'
        p["read_during_write_mode_port_b"] = '"NEW_DATA_NO_NBE_READ"'
        conns.update({"wren_b": "KEY[1]" if not aclr else "SW[17]", "data_b": bus(wb, 7), "q_a": "qa"})
    if cfg.get("outreg_b", "none") != "none" and clken:
        p["outdata_aclr_b"] = '"NONE"'
if aclr:
    conns["aclr0"] = "clr" if aclr == "logic" else "KEY[1]"
    if cfg.get("outreg_a", "none") != "none":
        p["outdata_aclr_a"] = '"CLEAR0"'
    if cfg.get("outreg_b", "none") != "none":
        p["outdata_aclr_b"] = '"CLEAR0"'
if byteena:
    p["width_byteena_a"] = nbe
    p["byte_size"] = 9 if wa % 9 == 0 else 8
    conns["byteena_a"] = "{" + ", ".join(f"SW[{10 + i}]" for i in range(nbe)) + "}"

v.append("  altsyncram ram (" + ", ".join(f".{k}({c})" for k, c in conns.items()) + ");")
v += [f"  defparam ram.{k} = {val};" for k, val in p.items()]
# Fold the outputs onto LEDs; every q bit reaches a pin.
outs = []
if "q_a" in conns:
    outs += [f"qa[{i}]" for i in range(wa)]
if "q_b" in conns:
    outs += [f"qb[{i}]" for i in range(wb)]
groups = [[] for _ in range(27)]
for i, o in enumerate(outs):
    groups[i % 27].append(o)
for i, g in enumerate(groups):
    rhs = " ^ ".join(g) if g else "1'b0"
    tgt = f"LEDR[{i}]" if i < 18 else f"LEDG[{i - 18}]"
    v.append(f"  assign {tgt} = {rhs};")
v.append("endmodule")
(out / "top.v").write_text("\n".join(v) + "\n")

qsf = [l for l in (pathlib.Path(__file__).parent.parent / "examples/de2.qsf").read_text().splitlines()
       if l.startswith("set_location_assignment")]
if cfg.get("loc"):
    qsf.append(f"set_location_assignment {cfg['loc']} -to \"altsyncram:ram|altsyncram_*:auto_generated|ram_block1a0\"")
(out / "assignments.qsf").write_text("\n".join(qsf) + "\n")
(out / "config.json").write_text(json.dumps(cfg) + "\n")
