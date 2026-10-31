#!/usr/bin/env python3
"""Generate a clock fuzz design for tools/oracle.sh: global clocks driven from
the core (a register, the JTAG block's TCK) and PLLs.

    gen_clk.py <out-dir> <json config>

Config keys:
  kind     "div"   CLOCK_50 divided by a register, that register clocking a
                   counter (`gclk`: forced onto a global clock)
           "tck"   the JTAG block's TCK clocking a 64-bit USER1 register
                   (examples/de2-jtag-counter's shape, clocked on TCK)
           "ena"   a cycloneii_clkctrl with its enable from a switch, on
                   CLOCK_50 (`src`: "pin") or a divided clock ("core")
           "none"  just the CLOCK_50 pin (for `data`)
           "pll"   one altpll (keys below), each used output clocking a
                   counter shown on the LEDs
  at       clock control location for div/ena, e.g. "CLKCTRL_G9"
  pll keys:
    clk      input pin: "CLOCK_50" (N2), "CLOCK_27" (D13), "EXT_CLOCK" (P26)
    outs     list of {"mul": m, "div": d, "phase": ps (string, ps),
             "duty": %} for c0, c1, c2 (null skips that output)
    mode     altpll operation_mode: "NORMAL" (default), "NO_COMPENSATION"
    cnt      false: no counters on the outputs (they drive only `data`)
    locked   lock output to LEDG[0]
    areset   areset from KEY[0] (inverted, active-low key)
    pfdena   pfdena from SW[17]
    extout   c2 also drives a pin through the PLL's dedicated clock out
             (pin name, e.g. "PIN_E15"; the output named `PLL_OUT`)
    at       PLL location, e.g. "PLL_1"
    atom     instantiate cycloneii_pll with these parameters instead of
             altpll (outs then only says which outputs get a counter)
  data     a global clock used as data, after any kind:
           {"src": expr (e.g. "c[0]", "CLOCK_50"), "sinks": [...]}, each sink
             {"pin": "PIN_B8"}           output GDATA<i> = src on that pin
             {"lut": "and"|"xor"|"mux"}  a LEDG pin = src & SW[0], src ^ SW[0],
                                         SW[0] ? src : SW[1]
             {"inv": "PIN_B8"}           output GDATA<i> = ~src
             {"reg": clk}                a register samples src on posedge clk,
                                         shown on a LEDG pin
"""
import json, pathlib, sys

LEDR = ["AE23", "AF23", "AB21", "AC22", "AD22", "AD23", "AD21", "AC21", "AA14",
        "Y13", "AA13", "AC14", "AD15", "AE15", "AF13", "AE13", "AE12", "AD12"]
LEDG = ["AE22", "AF22", "W19", "V18", "U18", "U17", "AA20", "Y18", "Y12"]
SW = {0: "N25", 1: "N26", 2: "P25", 17: "V2"}
KEY = {0: "G26", 1: "N23"}
CLK = {"CLOCK_50": ("N2", 20000), "CLOCK_27": ("D13", 37037), "EXT_CLOCK": ("P26", 20000)}

out = pathlib.Path(sys.argv[1])
cfg = json.loads(sys.argv[2])
kind = cfg["kind"]
v, qsf = [], []
ports = set()


def led(bits, prefix="LEDR"):
    """Show `bits` (list of expressions) on LEDR/LEDG pins."""
    pins = LEDR if prefix == "LEDR" else LEDG
    for i, b in enumerate(bits):
        v.append(f"  assign {prefix}[{i}] = {b};")
        qsf.append(f"set_location_assignment PIN_{pins[i]} -to {prefix}[{i}]")
    ports.add(f"output [{len(pins) - 1}:0] {prefix}")


def pin_in(name, pin):
    ports.add(f"input {name}")
    qsf.append(f"set_location_assignment PIN_{pin} -to {name}")


if kind == "div":
    pin_in("CLOCK_50", "N2")
    v += ["  reg [3:0] div = 0;",
          "  always @(posedge CLOCK_50) div <= div + 1'b1;",
          "  reg [25:0] cnt = 0;",
          "  always @(posedge div[3]) cnt <= cnt + 1'b1;"]
    led([f"cnt[{25 - i}]" for i in range(18)])
    if cfg.get("gclk", True):
        qsf.append('set_instance_assignment -name GLOBAL_SIGNAL "GLOBAL CLOCK" -to div[3]')
    if "at" in cfg:
        qsf.append(f"set_location_assignment {cfg['at']} -to div[3]~clkctrl")
elif kind == "tck":
    pin_in("CLOCK_50", "N2")
    for p in ["tms", "tck", "tdi"]:
        ports.add(f"input altera_reserved_{p}")
    ports.add("output altera_reserved_tdo")
    v += ["  reg [31:0] count = 0, gray = 0;",
          "  always @(posedge CLOCK_50) begin count <= count + 1'b1; gray <= count ^ (count >> 1); end",
          "  wire tck, tdi, shift, usr1;",
          "  reg [63:0] dr = 0;",
          "  cycloneii_jtag jtag (.tms(altera_reserved_tms), .tck(altera_reserved_tck), .tdi(altera_reserved_tdi),",
          "    .tdo(altera_reserved_tdo), .tckutap(tck), .tdiutap(tdi), .shiftuser(shift), .usr1user(usr1),",
          "    .tdouser(dr[0]));",
          f"  always @({'negedge' if cfg.get('neg') else 'posedge'} tck)",
          "    if (shift & usr1) dr <= {tdi, dr[63:1]};",
          "    else dr <= {32'h44453221, gray};"]
    led([f"count[{26 - i}]" for i in range(4)])
elif kind == "ena":
    pin_in("CLOCK_50", "N2")
    pin_in("SW[0]", SW[0])
    ports.discard("input SW[0]")
    ports.add("input [0:0] SW")
    if cfg.get("src", "pin") == "core":
        v += ["  reg [3:0] div = 0;",
              "  always @(posedge CLOCK_50) div <= div + 1'b1;",
              "  wire src = div[3];"]
    else:
        v += ["  wire src = CLOCK_50;"]
    v += ["  wire gclk;",
          "  cycloneii_clkctrl #(.clock_type(\"global clock\"), .ena_register_mode(\"" + cfg.get("ereg", "falling edge") + "\"))",
          "    cc (.inclk({3'b000, src}), .clkselect(2'b00), .ena(SW[0]), .outclk(gclk));",
          "  reg [25:0] cnt = 0;",
          "  always @(posedge gclk) cnt <= cnt + 1'b1;"]
    led([f"cnt[{25 - i}]" for i in range(18)])
    if "at" in cfg:
        qsf.append(f"set_location_assignment {cfg['at']} -to cc")
elif kind == "pll":
    cname = cfg.get("clk", "CLOCK_50")
    pin, period = CLK[cname]
    pin_in(cname, pin)
    outs = cfg["outs"] + [None] * (3 - len(cfg["outs"]))
    p = {"intended_device_family": '"Cyclone II"', "lpm_type": '"altpll"',
         "operation_mode": f'"{cfg.get("mode", "NORMAL")}"',
         "inclk0_input_frequency": str(period), "port_inclk0": '"PORT_USED"',
         "port_inclk1": '"PORT_UNUSED"'}
    if cfg.get("mode", "NORMAL") == "NORMAL":
        p["compensate_clock"] = f'"CLK{next(i for i, o in enumerate(outs) if o)}"'
    for i, o in enumerate(outs):
        p[f"port_clk{i}"] = '"PORT_USED"' if o else '"PORT_UNUSED"'
        if o:
            p[f"clk{i}_multiply_by"] = str(o.get("mul", 1))
            p[f"clk{i}_divide_by"] = str(o.get("div", 1))
            p[f"clk{i}_phase_shift"] = f'"{o.get("phase", "0")}"'
            p[f"clk{i}_duty_cycle"] = str(o.get("duty", 50))
    for port in ["locked", "areset", "pfdena"]:
        p[f"port_{port}"] = '"PORT_USED"' if cfg.get(port) else '"PORT_UNUSED"'
    for port in ["clk3", "clk4", "clk5", "clkena0", "clkena1", "clkena2", "clkena3", "clkena4",
                 "clkena5", "extclk0", "extclk1", "extclk2", "extclk3", "activeclock", "clkbad0",
                 "clkbad1", "clkloss", "clkswitch", "fbin", "phasecounterselect", "phasedone",
                 "phasestep", "phaseupdown", "scanaclr", "scanclk", "scanclkena", "scandata",
                 "scandataout", "scandone", "scanread", "scanwrite", "configupdate"]:
        p[f"port_{port}"] = '"PORT_UNUSED"'
    conns = [f".inclk({{1'b0, {cname}}})", ".clk(c)"]
    if cfg.get("locked"):
        conns.append(".locked(locked)")
        v.append("  wire locked;")
    if cfg.get("areset"):
        pin_in("KEY[0]", KEY[0])
        ports.discard("input KEY[0]")
        ports.add("input [0:0] KEY")
        conns.append(".areset(!KEY[0])")
    if cfg.get("pfdena"):
        pin_in("SW[17]", SW[17])
        ports.discard("input SW[17]")
        ports.add("input [17:17] SW")
        conns.append(".pfdena(SW[17])")
    v.append("  wire [5:0] c;")
    if "atom" in cfg:
        # The WYSIWYG atom with the given parameters (what an exported
        # netlist carries), not altpll.
        conns = [c.replace(".clk(c)", ".clk(c[2:0])") for c in conns]
        a = {"operation_mode": '"normal"', "inclk0_input_frequency": str(period), "lpm_type": '"cycloneii_pll"'}
        a.update({k: (f'"{x}"' if isinstance(x, str) else str(x)) for k, x in cfg["atom"].items()})
        v.append("  cycloneii_pll #(" + ", ".join(f".{k}({x})" for k, x in a.items()) + ")")
    else:
        v.append("  altpll #(" + ", ".join(f".{k}({x})" for k, x in p.items()) + ")")
    v.append("    pll (" + ", ".join(conns) + ");")
    leds = []
    for i, o in enumerate(outs):
        if o and cfg.get("cnt", True):
            v += [f"  reg [23:0] cnt{i} = 0;", f"  always @(posedge c[{i}]) cnt{i} <= cnt{i} + 1'b1;"]
            leds += [f"cnt{i}[{23 - k}]" for k in range(6)]
    led(leds)
    if cfg.get("locked"):
        led(["locked"], "LEDG")
    if cfg.get("extout"):
        ports.add("output PLL_OUT")
        v.append("  assign PLL_OUT = c[2];")
        qsf.append(f"set_location_assignment {cfg['extout']} -to PLL_OUT")
    if "at" in cfg:
        qsf.append(f"set_location_assignment {cfg['at']} -to {'pll' if 'atom' in cfg else 'pll|pll'}")
elif kind == "none":
    pin_in("CLOCK_50", "N2")
else:
    sys.exit(f"unknown kind {kind}")

if "data" in cfg:
    src = cfg["data"]["src"]
    for i, sk in enumerate(cfg["data"]["sinks"]):
        if "pin" in sk or "inv" in sk:
            ports.add(f"output GDATA{i}")
            v.append(f"  assign GDATA{i} = {'~' if 'inv' in sk else ''}{src};")
            qsf.append(f"set_location_assignment {sk.get('pin') or sk['inv']} -to GDATA{i}")
            continue
        for k in (0, 1):
            pin_in(f"SW[{k}]", SW[k])
            ports.discard(f"input SW[{k}]")
        ports.discard("input [0:0] SW")
        ports.add("input [1:0] SW")
        if "lut" in sk:
            e = {"and": f"{src} & SW[0]", "xor": f"{src} ^ SW[0]", "mux": f"SW[0] ? {src} : SW[1]"}[sk["lut"]]
        else:
            v += [f"  reg gd{i} = 0;", f"  always @(posedge {sk['reg']}) gd{i} <= {src};"]
            e = f"gd{i}"
        ports.add(f"output GLED{i}")
        v.append(f"  assign GLED{i} = {e};")
        qsf.append(f"set_location_assignment PIN_{LEDG[1 + i]} -to GLED{i}")

out.mkdir(parents=True, exist_ok=True)
(out / "top.v").write_text(f"module top({', '.join(sorted(ports))});\n" + "\n".join(v) + "\nendmodule\n")
(out / "assignments.qsf").write_text("\n".join(qsf) + "\n")
(out / "config.json").write_text(json.dumps(cfg) + "\n")
