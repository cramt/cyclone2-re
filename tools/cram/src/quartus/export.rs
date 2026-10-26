//! A routed nextpnr design (`bitstream::nextpnr::Routed`) as Quartus
//! constraints: an atom netlist, location assignments and an rcf, so the
//! oracle can assemble exactly this placement and routing
//! (`cram quartus-export`, tools/validate.sh).

use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use chipsmith_cyclone2::bitstream::device::{element_name, Device};
use chipsmith_cyclone2::bitstream::hard::HardInput;
use chipsmith_cyclone2::bitstream::io::{IoDb, BLOCK_COL};
use chipsmith_cyclone2::bitstream::m4k;
use chipsmith_cyclone2::bitstream::pll::Feedback;
use chipsmith_cyclone2::bitstream::nextpnr::{flag, lut_mask, perms, pin_at, split_port, Bel, Cell, IoePin, LePin, M4kPin, M4kShape, PortDir, Routed, Wire};
use chipsmith_cyclone2::pin::{assignment, register_param};

/// Output pins of the CY2_JTAG and CY2_ASMI cells.
const HARD_OUTPUTS: [&str; 9] =
    ["TMSUTAP", "TCKUTAP", "TDIUTAP", "SHIFTUSER", "CLKDRUSER", "UPDATEUSER", "RUNIDLEUSER", "USR1USER", "DATA0OUT"];
/// The dedicated JTAG pins, which a VQM instantiating the JTAG block has
/// to connect it to (quartus_map: "JTAG port tck must connect to an I/O
/// pin").
const JTAG_PORTS: [(&str, &str, &str); 4] = [
    ("input", "altera_reserved_tms", "tms"),
    ("input", "altera_reserved_tck", "tck"),
    ("input", "altera_reserved_tdi", "tdi"),
    ("output", "altera_reserved_tdo", "tdo"),
];

fn verilog(d: PortDir) -> &'static str {
    match d {
        PortDir::In => "input",
        PortDir::Out => "output",
        PortDir::InOut => "inout",
    }
}

/// A read-only x36: Quartus' "rom" atom, which it builds on port B
/// (fuzz r36), so its port A address enters on port B's pins.
fn rom36(shape: &M4kShape) -> bool {
    shape.dual && !shape.writes
}

/// Quartus atom port (`PORTAADDR[i]`, ...) of each physical input.
fn atom_ports(shape: &M4kShape) -> Result<HashMap<m4k::Input, String>> {
    let n = shape.width.addr_bits();
    let mut out = HashMap::new();
    for i in 0..n {
        out.insert(m4k::Input::addr(rom36(shape), 12 - n + i)?, format!("PORTAADDR[{i}]"));
        if shape.dual && shape.writes {
            out.insert(m4k::Input::addr(true, 12 - n + i)?, format!("PORTBADDR[{i}]"));
        }
    }
    if shape.writes {
        for k in 0..shape.bits {
            out.insert(m4k::Input::Data(k as u8), format!("PORTADATAIN[{k}]"));
        }
    }
    Ok(out)
}

/// Escaped Verilog identifier when needed.
fn vid(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !s.starts_with(|c: char| c.is_ascii_digit()) {
        s.to_string()
    } else {
        format!("\\{s} ")
    }
}

fn resource(dev: &Device, w: Wire, ioe_out_x: impl Fn(u32, u32, u32) -> u32) -> Option<String> {
    match w {
        Wire::Node(n) => {
            let e = &dev.routing.elements[n.0 as usize];
            let t = &dev.routing.templates[e.template as usize];
            // The chip database's names are element_name's; the rcf's are
            // route::element_enum's (M4K data outputs: M4K_BUFFER).
            let kind = if t.element_enum == 278 { "M4K_BUFFER".to_string() } else { element_name(t.element_enum) };
            Some(format!("{kind}:X{}Y{}S{}I{}", e.x, e.y, t.z, t.index))
        }
        Wire::M4k { x, y, pin: M4kPin::Ctl(k) } => Some(format!("M4K_LEFT_CONTROL_INPUT:X{x}Y{y}S0I{k}")),
        Wire::LocalLine { x, y, k } => Some(format!("LOCAL_LINE:X{x}Y{y}S0I{k}")),
        Wire::Ctrl { x, y, k } => Some(format!("LAB_CONTROL_MUX:X{x}Y{y}S0I{k}")),
        Wire::Ioe { x, y, z, pin: IoePin::DataOut } => Some(format!("IO_DATAOUT:X{}Y{y}S{}I0", ioe_out_x(x, y, z), z % 8)),
        Wire::Ioe { x, y, z, pin: IoePin::Oe } => Some(format!("IO_OE_PIN:X{}Y{y}S{}I0", ioe_out_x(x, y, z), z % 8)),
        Wire::PllIn { x, y, input } => {
            let kind = match input {
                chipsmith_cyclone2::bitstream::pll::Control::Areset => "PLL_ARESET_PIN",
                chipsmith_cyclone2::bitstream::pll::Control::Pfdena => "PLL_PFD_ENA_PIN",
            };
            Some(format!("{kind}:X{x}Y{y}S0I0"))
        }
        Wire::Hard(h) => {
            let (x, y) = h.xy();
            let (kind, z) = match h {
                HardInput::Tdouser => ("JTAG_TDOUSER_PIN", 0),
                HardInput::Dclk => ("ASMI_DCLKIN", 2),
                HardInput::Sce => ("ASMI_SCEIN", 2),
                HardInput::Sdo => ("ASMI_SDOIN", 2),
            };
            Some(format!("{kind}:X{x}Y{y}S{z}I0"))
        }
        _ => None,
    }
}

/// The routed design as Quartus input: `top.vqm` (an atom netlist, so
/// quartus_map keeps every cell as is), location assignments and the
/// routing as an rcf. Cells are renamed after their location.
pub struct QuartusExport {
    pub vqm: String,
    pub qsf: String,
    pub rcf: String,
}

pub fn to_quartus(r: &Routed, dev: &Device, io: &IoDb) -> Result<QuartusExport> {
    // Atom names.
    let mut atom: HashMap<&str, String> = HashMap::new();
    let mut port_of: HashMap<&str, String> = HashMap::new();
    for (name, c) in &r.cells {
        let a = match c.bel {
            Bel::Lut { x, y, k } => format!("lc_x{x}_y{y}_n{}", 2 * k),
            Bel::Ff { x, y, k } => format!("ff_x{x}_y{y}_n{}", 2 * k + 1),
            Bel::ClkCtrl { g } => format!("clkctrl_g{g}"),
            Bel::M4k { x, y } => format!("m4k_x{x}_y{y}"),
            Bel::Jtag => "jtag".to_string(),
            Bel::Asmi => "asmi".to_string(),
            Bel::Pll { x, y } => format!("pll_x{x}_y{y}"),
            Bel::Ioe { .. } => {
                let p = c.attrs.get("PORT").with_context(|| format!("{name}: no PORT"))?;
                port_of.insert(name, p.clone());
                p.clone()
            }
        };
        atom.insert(name, a);
    }
    // Net id -> driver signal name (what the rcf and the netlist call it).
    let mut driver: HashMap<u64, String> = HashMap::new();
    for (name, c) in &r.cells {
        let a = &atom[name.as_str()];
        for (p, &id) in &c.conns {
            let sig = match (c.ty.as_str(), p.as_str()) {
                ("LCELL_COMB", "COMBOUT") => format!("{a}_combout"),
                ("LCELL_COMB", "COUT") => format!("{a}_cout"),
                ("LCELL_FF", "REGOUT") => format!("{a}_regout"),
                ("CLKCTRL", "OUTCLK") => format!("{a}_outclk"),
                // A plain input is a port of the netlist; an input with its
                // register is an I/O atom, like any other pin.
                ("CY2_IBUF", "COMBOUT") if !c.params.contains_key(register_param::INPUT) => a.clone(),
                ("CY2_IBUF" | "CY2_IOBUF", "COMBOUT") => format!("{a}_combout"),
                ("CY2_IBUF" | "CY2_IOBUF", "REGOUT") => format!("{a}_regout"),
                ("CY2_M4K", o) if o.starts_with("DO") => format!("{a}_q{}", &o[2..]),
                ("CY2_JTAG", o) if HARD_OUTPUTS.contains(&o) => format!("{a}_{}", o.to_lowercase()),
                ("CY2_ASMI", "DATA0OUT") => format!("{a}_data0out"),
                ("CY2_PLL", o @ ("CLK0" | "CLK1" | "CLK2" | "LOCKED")) => format!("{a}_{}", o.to_lowercase()),
                _ => continue,
            };
            driver.insert(id, sig);
        }
    }
    // Feeders: a one-input pass-through (or inverting) LUT whose only
    // load is its own LE's register. quartus_map deletes such cells
    // and the fitter puts its own `~feeder` in the same LE, so the
    // netlist connects the register straight to the source and the rcf
    // names the physical LUT input as the register's route_port.
    let mut loads: HashMap<u64, Vec<(&str, &str)>> = HashMap::new();
    for (name, c) in &r.cells {
        for (p, &id) in &c.conns {
            let ram_out = (c.ty == "CY2_M4K" && p.starts_with("DO"))
                || HARD_OUTPUTS.contains(&p.as_str())
                || (c.ty == "CY2_PLL" && ["CLK0", "CLK1", "CLK2", "LOCKED"].contains(&p.as_str()));
            if !["COMBOUT", "COUT", "REGOUT", "OUTCLK"].contains(&p.as_str()) && !(c.ty == "CY2_IBUF" && p != "INCLK") && !ram_out {
                loads.entry(id).or_default().push((name.as_str(), p.as_str()));
            }
        }
    }
    let cell_at: HashMap<Bel, &str> = r.cells.iter().map(|(n, c)| (c.bel, n.as_str())).collect();
    // LUT cell -> (source net, inverted)
    let mut feeder: HashMap<&str, (u64, bool)> = HashMap::new();
    for (name, c) in &r.cells {
        let Bel::Lut { x, y, k } = c.bel else { continue };
        let ins: Vec<(usize, u64)> = ["DATAA", "DATAB", "DATAC", "DATAD"]
            .iter()
            .enumerate()
            .filter_map(|(i, p)| c.conns.get(*p).map(|&id| (i, id)))
            .collect();
        let [(i, src)] = ins[..] else { continue };
        if c.conns.contains_key("CIN") || c.conns.get("COUT").is_some_and(|id| loads.contains_key(id)) {
            continue;
        }
        let buf = [0xAAAAu16, 0xCCCC, 0xF0F0, 0xFF00][i];
        let m = lut_mask(c)?;
        if m != buf && m != !buf {
            continue;
        }
        let Some(&out) = c.conns.get("COMBOUT") else { continue };
        let Some(&ff) = cell_at.get(&Bel::Ff { x, y, k }) else { continue };
        if loads.get(&out).map(Vec::as_slice) == Some(&[(ff, "DATAIN")][..]) {
            feeder.insert(name.as_str(), (src, m != buf));
        }
    }
    // Inverted copies of nets (`assign n_inv = ~n`), which Quartus folds
    // into the load: an output pin's or a LAB control's inversion.
    let mut inverted: BTreeMap<String, String> = BTreeMap::new();
    let mut invert = |sig: String| -> String {
        let w = format!("{sig}_inv");
        inverted.insert(sig, w.clone());
        vid(&w)
    };
    let net = |c: &Cell, p: &str, dflt: &str| -> String {
        c.conns.get(p).and_then(|id| driver.get(id)).map(|s| vid(s)).unwrap_or_else(|| dflt.to_string())
    };
    let sig_of = |c: &Cell, p: &str| -> Option<String> { c.conns.get(p).and_then(|id| driver.get(id)).cloned() };

    // Netlist.
    let mut ports: BTreeMap<String, (&str, Option<u32>)> = BTreeMap::new();
    for p in port_of.values() {
        let (b, i) = split_port(p);
        let e = ports.entry(b.to_string()).or_insert((verilog(r.port_dir(p)?), None));
        if let Some(i) = i {
            e.1 = Some(e.1.map_or(i, |m: u32| m.max(i)));
        }
    }
    let jtag = r.cells.values().any(|c| c.bel == Bel::Jtag);
    let mut v = String::new();
    writeln!(v, "// Generated by `cram quartus-export` from a nextpnr routed design.")?;
    let mut names: Vec<String> = ports.keys().map(|p| vid(p)).collect();
    if jtag {
        names.extend(JTAG_PORTS.iter().map(|p| p.1.to_string()));
    }
    writeln!(v, "module top({});", names.join(", "))?;
    for (p, (out, w)) in &ports {
        let r = w.map(|m| format!("[{m}:0] ")).unwrap_or_default();
        writeln!(v, "{out} {r}{};", vid(p))?;
    }
    if jtag {
        for (dir, p, _) in JTAG_PORTS {
            writeln!(v, "{dir} {p};")?;
        }
    }
    let feeder_outs: std::collections::HashSet<&String> =
        feeder.keys().filter_map(|n| r.cells[*n].conns.get("COMBOUT").and_then(|id| driver.get(id))).collect();
    let mut sigs: Vec<&String> =
        driver.values().filter(|s| !port_of.values().any(|p| p == *s) && !feeder_outs.contains(s)).collect();
    sigs.sort();
    for s in sigs {
        writeln!(v, "wire {};", vid(s))?;
    }
    let mut qsf = String::new();
    // Keep every atom as placed: by default quartus_map drops single-
    // input LUTs ("redundant logic cells") and the fitter re-adds its
    // own feeders and folds inverters into neighbours.
    for a in ["REMOVE_REDUNDANT_USER_CELLS", "REMOVE_REDUNDANT_LOGIC_CELLS", "REMOVE_DUPLICATE_REGISTERS", "REMOVE_DUPLICATE_LOGIC"] {
        writeln!(qsf, "set_global_assignment -name {a} OFF")?;
    }
    // The DE2 powers every bank at 3.3 V; Quartus would otherwise take a
    // bank's VCCIO from its pins (2.5 V for one holding only 2.5-V
    // inputs), and its unclaimed dual-purpose pins with it.
    for bank in 1..=8 {
        writeln!(qsf, "set_global_assignment -name IOBANK_VCCIO 3.3V -section_id {bank}")?;
    }
    if let Some(mode) = r.attrs.get(assignment::UNUSED_PINS) {
        writeln!(qsf, "set_global_assignment -name {} \"{mode}\"", assignment::UNUSED_PINS)?;
    }
    writeln!(qsf, "set_global_assignment -name ROUTING_BACK_ANNOTATION_MODE NORMAL")?;
    writeln!(qsf, "set_global_assignment -name ROUTING_BACK_ANNOTATION_FILE ../routing.rcf")?;
    for (name, c) in &r.cells {
        let a = vid(&atom[name.as_str()]);
        match c.bel {
            Bel::Lut { .. } if feeder.contains_key(name.as_str()) => {}
            Bel::Lut { x, y, k } => {
                let cin = c.conns.contains_key("CIN");
                writeln!(v, "cycloneii_lcell_comb {a}(")?;
                for (port, d) in [("dataa", "DATAA"), ("datab", "DATAB"), ("datac", "DATAC"), ("datad", "DATAD")] {
                    writeln!(v, "  .{port}({}),", net(c, d, "1'b1"))?;
                }
                writeln!(v, "  .cin({}),", net(c, "CIN", "1'b0"))?;
                writeln!(v, "  .combout({}),", net(c, "COMBOUT", ""))?;
                writeln!(v, "  .cout({}));", net(c, "COUT", ""))?;
                writeln!(v, "defparam {a}.lut_mask = 16'h{:04X};", lut_mask(c)?)?;
                writeln!(v, "defparam {a}.sum_lutc_input = \"{}\";", if cin { "cin" } else { "datac" })?;
                writeln!(qsf, "set_location_assignment LCCOMB_X{x}_Y{y}_N{} -to {}", 2 * k, atom[name.as_str()])?;
            }
            Bel::Ff { x, y, k } => {
                writeln!(v, "cycloneii_lcell_ff {a}(")?;
                writeln!(v, "  .clk({}),", net(c, "CLK", "1'b0"))?;
                let d = match cell_at.get(&Bel::Lut { x, y, k }).and_then(|l| feeder.get(l)) {
                    Some(&(src, inv)) => {
                        let s = driver.get(&src).context("feeder source")?.clone();
                        if inv { invert(s) } else { vid(&s) }
                    }
                    None => net(c, "DATAIN", "1'b0"),
                };
                let ctl = |p: &str, inv: &str, dflt: &str, invert: &mut dyn FnMut(String) -> String| match sig_of(c, p) {
                    Some(s) if flag(c, inv) => invert(s),
                    Some(s) => vid(&s),
                    None => dflt.to_string(),
                };
                let ena = ctl("ENA", "ENA_INV", "1'b1", &mut invert);
                let sclr = ctl("SCLR", "SCLR_INV", "1'b0", &mut invert);
                writeln!(v, "  .datain({d}),")?;
                writeln!(v, "  .ena({ena}), .aclr({}), .sclr({sclr}),", net(c, "ACLR", "1'b0"))?;
                let sload = if c.conns.contains_key("SDATA") { "1'b1" } else { "1'b0" };
                writeln!(v, "  .sdata({}), .sload({sload}),", net(c, "SDATA", "1'b0"))?;
                writeln!(v, "  .regout({}));", net(c, "REGOUT", ""))?;
                writeln!(qsf, "set_location_assignment LCFF_X{x}_Y{y}_N{} -to {}", 2 * k + 1, atom[name.as_str()])?;
            }
            Bel::M4k { x, y } => {
                let shape = M4kShape::of(c).with_context(|| name.clone())?;
                let n = shape.width.addr_bits();
                let (w, depth) = (shape.bits, 1u32 << n);
                let pin = |inp: m4k::Input| net(c, &inp.pin_name(), "1'b0");
                let addr = |b: bool| -> Result<String> {
                    let v: Vec<String> = (0..n).rev().map(|i| Ok(pin(m4k::Input::addr(b, 12 - n + i)?))).collect::<Result<_>>()?;
                    Ok(format!("{{{}}}", v.join(", ")))
                };
                let outs: Vec<String> = (0..w)
                    .rev()
                    .map(|k| match sig_of(c, &format!("DO{k}")) {
                        Some(s) => vid(&s),
                        None => {
                            let u = format!("{}_unused_q{k}", atom[name.as_str()]);
                            writeln!(v, "wire {};", vid(&u)).unwrap();
                            vid(&u)
                        }
                    })
                    .collect();
                let op = match (shape.dual, shape.writes) {
                    (false, true) => "single_port",
                    (true, true) => "dual_port",
                    (_, false) => "rom",
                };
                writeln!(v, "cycloneii_ram_block {a}(")?;
                writeln!(v, "  .clk0({}),", net(c, "CLK0", "1'b0"))?;
                // A dual port must have port B's read enable driven
                // (error 14091), the other modes must not (14090).
                if op == "dual_port" {
                    writeln!(v, "  .portbrewe(1'b1),")?;
                }
                writeln!(v, "  .portaaddr({}),", addr(rom36(&shape))?)?;
                if shape.writes {
                    writeln!(v, "  .portawe({}),", net(c, "WE", "1'b0"))?;
                    let d: Vec<String> = (0..w).rev().map(|k| pin(m4k::Input::Data(k as u8))).collect();
                    writeln!(v, "  .portadatain({{{}}}),", d.join(", "))?;
                }
                let out_port = if shape.dual && shape.writes {
                    writeln!(v, "  .portbaddr({}),", addr(true)?)?;
                    "portbdataout"
                } else {
                    "portadataout"
                };
                writeln!(v, "  .{out_port}({{{}}}));", outs.join(", "))?;
                let mut p: Vec<(String, String)> = vec![
                    ("operation_mode".into(), format!("\"{op}\"")),
                    ("ram_block_type".into(), "\"M4K\"".into()),
                    ("logical_ram_name".into(), format!("\"{}\"", atom[name.as_str()])),
                    ("init_file_layout".into(), "\"port_a\"".into()),
                ];
                let ports: &[char] = if shape.dual && shape.writes { &['a', 'b'] } else { &['a'] };
                for &pt in ports {
                    for (k, val) in [
                        ("address_width", n.to_string()),
                        ("data_width", w.to_string()),
                        ("first_address", "0".into()),
                        ("last_address", (depth - 1).to_string()),
                        ("logical_ram_depth", depth.to_string()),
                        ("logical_ram_width", w.to_string()),
                        ("first_bit_number", "0".into()),
                        ("data_out_clock", "\"none\"".into()),
                    ] {
                        p.push((format!("port_{pt}_{k}"), val));
                    }
                }
                if ports.len() == 2 {
                    p.push(("port_b_address_clock".into(), "\"clock0\"".into()));
                    p.push(("port_b_read_enable_write_enable_clock".into(), "\"clock0\"".into()));
                    p.push(("mixed_port_feed_through_mode".into(), "\"dont_care\"".into()));
                }
                // Contents in the words of port A: word a bit k at bit
                // a * w + k, 2,048 bits per mem_init parameter (as
                // Quartus writes them).
                let mut bits = Vec::with_capacity((depth * w) as usize);
                for a in 0..depth as usize {
                    for k in 0..w as usize {
                        bits.push(shape.content.get_linear(shape.width.linear(a, k)));
                    }
                }
                for (i, chunk) in bits.chunks(2048).enumerate() {
                    let hex: String = chunk
                        .rchunks(4)
                        .map(|q| {
                            let d = q.iter().enumerate().fold(0u32, |acc, (j, &b)| acc | (b as u32) << j);
                            char::from_digit(d, 16).unwrap().to_ascii_uppercase()
                        })
                        .collect();
                    p.push((format!("mem_init{i}"), format!("{}'h{hex}", chunk.len())));
                }
                for (k, val) in p {
                    writeln!(v, "defparam {a}.{k} = {val};")?;
                }
                writeln!(qsf, "set_location_assignment M4K_X{x}_Y{y} -to {}", atom[name.as_str()])?;
            }
            Bel::Pll { .. } => {
                // The atom with its counters; the fitter derives the loop
                // settings from them (cyclone2-re README, "PLLs": a1).
                let used = [0, 1, 2].map(|k| c.conns.contains_key(&format!("CLK{k}")));
                let req = chipsmith_cyclone2::pll::Request::from_params(&c.params, used)?;
                let cfg = req.solve().with_context(|| name.clone())?.config;
                let w = |s: Option<String>| s.map(|s| vid(&s)).unwrap_or_default();
                for k in 0..3 {
                    if !used[k] {
                        writeln!(v, "wire {};", vid(&format!("{}_nc{k}", atom[name.as_str()])))?;
                    }
                }
                writeln!(v, "cycloneii_pll {a}(")?;
                writeln!(v, "  .inclk({{1'b0, {}}}),", net(c, "INCLK", "1'b0"))?;
                writeln!(v, "  .areset({}),", net(c, "ARESET", "1'b0"))?;
                writeln!(v, "  .pfdena({}),", net(c, "PFDENA", "1'b1"))?;
                // Unused outputs still need a wire in the concatenation.
                let mut clks = Vec::new();
                for k in (0..3).rev() {
                    clks.push(match sig_of(c, &format!("CLK{k}")) {
                        Some(sig) => vid(&sig),
                        None => {
                            let nc = format!("{}_nc{k}", atom[name.as_str()]);
                            vid(&nc)
                        }
                    });
                }
                writeln!(v, "  .clk({{{}}}),", clks.join(", "))?;
                writeln!(v, "  .locked({}));", w(sig_of(c, "LOCKED")))?;
                let mut p: Vec<(String, String)> = vec![
                    ("operation_mode".into(), format!("\"{}\"", match cfg.feedback { Feedback::Normal => "normal", Feedback::NoCompensation => "no_compensation" })),
                    ("inclk0_input_frequency".into(), req.inclk_period_ps.to_string()),
                    ("m".into(), cfg.m.count().to_string()),
                    ("n".into(), cfg.n.to_string()),
                    ("m_initial".into(), cfg.m_initial.to_string()),
                    ("m_ph".into(), cfg.m_phase.to_string()),
                    ("vco_post_scale".into(), if cfg.vco_div2 { "2" } else { "1" }.into()),
                ];
                for (k, o) in cfg.outputs.iter().enumerate() {
                    let Some(o) = o else { continue };
                    let ck = format!("c{k}");
                    match o.divider {
                        Some(d) => {
                            p.push((format!("{ck}_high"), d.high().to_string()));
                            p.push((format!("{ck}_low"), d.low().to_string()));
                            p.push((format!("{ck}_mode"), format!("\"{}\"", if d.odd() { "odd" } else { "even" })));
                        }
                        None => p.push((format!("{ck}_mode"), "\"bypass\"".into())),
                    }
                    p.push((format!("{ck}_initial"), o.initial.to_string()));
                    p.push((format!("{ck}_ph"), o.phase.to_string()));
                    let r = req.outputs[k].context("solved output without its request")?;
                    p.push((format!("clk{k}_counter"), format!("\"{ck}\"")));
                    p.push((format!("clk{k}_multiply_by"), r.multiply.to_string()));
                    p.push((format!("clk{k}_divide_by"), r.divide.to_string()));
                    p.push((format!("clk{k}_phase_shift"), format!("\"{}\"", r.phase_ps)));
                    p.push((format!("clk{k}_duty_cycle"), r.duty_percent.to_string()));
                }
                if cfg.feedback == Feedback::Normal {
                    let first = cfg.outputs.iter().position(Option::is_some).unwrap_or(0);
                    p.push(("compensate_clock".into(), format!("\"clk{first}\"")));
                }
                for (k, val) in p {
                    writeln!(v, "defparam {a}.{k} = {val};")?;
                }
                let site = chipsmith_cyclone2::bitstream::pll::Site::ALL
                    .into_iter()
                    .find(|s| Bel::Pll { x: s.xy().0, y: s.xy().1 } == c.bel)
                    .context("PLL bel at no PLL site")?;
                writeln!(qsf, "set_location_assignment {} -to {}", site.name(), atom[name.as_str()])?;
            }
            Bel::Jtag => {
                writeln!(v, "cycloneii_jtag {a}(")?;
                for (_, p, atom_port) in JTAG_PORTS {
                    writeln!(v, "  .{atom_port}({p}),")?;
                }
                let outs: Vec<String> =
                    HARD_OUTPUTS[..8].iter().map(|o| format!("  .{}({})", o.to_lowercase(), net(c, o, ""))).collect();
                writeln!(v, "  .tdouser({}),\n{});", net(c, "TDOUSER", "1'b0"), outs.join(",\n"))?;
            }
            Bel::Asmi => {
                writeln!(v, "cycloneii_asmiblock {a}(")?;
                for (p, cp) in [("dclkin", "DCLKIN"), ("scein", "SCEIN"), ("sdoin", "SDOIN")] {
                    writeln!(v, "  .{p}({}),", net(c, cp, "1'b0"))?;
                }
                writeln!(v, "  .oe(1'b0),\n  .data0out({}));", net(c, "DATA0OUT", ""))?;
            }
            Bel::ClkCtrl { g } => {
                writeln!(v, "cycloneii_clkctrl {a}(")?;
                writeln!(v, "  .inclk({{1'b0, 1'b0, 1'b0, {}}}),", net(c, "INCLK", "1'b0"))?;
                // No clkselect (the VQM reader rejects vector constants) and no
        // ena: a tied-on enable makes Quartus clear the clock control's
        // CLKBUF_ENABLE_AND_INVERT bit, which its own netlists (ena =
        // vcc) don't.
                writeln!(v, "  .outclk({}));", net(c, "OUTCLK", ""))?;
                writeln!(v, "defparam {a}.clock_type = \"global clock\";")?;
                // The atom default registers the enable, which uses
                // CLKBUF_ENABLE_AND_INVERT instead of CLKBUF_ENABLE_MODE.
                writeln!(v, "defparam {a}.ena_register_mode = \"none\";")?;
                writeln!(qsf, "set_location_assignment CLKCTRL_G{g} -to {}", atom[name.as_str()])?;
            }
            Bel::Ioe { x, y, z } => {
                let p = &atom[name.as_str()];
                writeln!(qsf, "set_location_assignment PIN_{} -to {p}", pin_at(io, x, y, z)?.name)?;
                // The pin's I/O settings, already Quartus assignments.
                for k in assignment::PIN {
                    if let Some(val) = c.params.get(k) {
                        writeln!(qsf, "set_instance_assignment -name {k} \"{val}\" -to {p}")?;
                    }
                }
                let cnst = |k: &String| if k.ends_with('1') { "1'b1".to_string() } else { "1'b0".to_string() };
                let bidir = r.port_dir(p)? == PortDir::InOut;
                let reg = |param: &str| c.params.get(param).map(String::as_str);
                let (in_reg, out_reg, oe_reg) = (reg(register_param::INPUT), reg(register_param::OUTPUT), reg(register_param::OE));
                if c.ty == "CY2_IBUF" && in_reg.is_none() {
                    // A port of the netlist.
                } else if c.ty == "CY2_OBUF" && !bidir && out_reg.is_none() {
                    let inv = if flag(c, "DATA_INVERT") { "~" } else { "" };
                    let d = match c.params.get("OUT_CONST") {
                        Some(k) => cnst(k),
                        None => format!("{inv}{}", net(c, "DATAOUT", "1'b0")),
                    };
                    writeln!(v, "assign {} = {d};", vid(p))?;
                } else {
                    // The VQM reader takes no `?:`, so a tri-state pin
                    // is an I/O atom, the way Quartus' own VQM writer
                    // has it; so is any pin of an inout port (Quartus
                    // wants it in bidir mode), and any pin using its
                    // registers. Inverted copies fold into the IOE
                    // (IO_DATA_INVERT, IO_OE_INVERT), as `__ALT_INV__`
                    // wires do in Quartus' VQMs.
                    let pin_net = |port: &str, const_param: &str, inv: &str, invert: &mut dyn FnMut(String) -> String| {
                        match (c.params.get(const_param), sig_of(c, port)) {
                            (Some(k), _) => Ok(cnst(k)),
                            (None, Some(s)) if flag(c, inv) => Ok(invert(s)),
                            (None, Some(s)) => Ok(vid(&s)),
                            (None, None) => Err(anyhow::anyhow!("{name}: {port} neither routed nor constant")),
                        }
                    };
                    // An inout that never drives is written the way
                    // Quartus maps one: an open drain with data 1.
                    let never = c.ty == "CY2_IOBUF" && c.params.get("OE_CONST").is_some_and(|k| !k.ends_with('1'));
                    let (d, oe) = if c.ty == "CY2_IBUF" {
                        ("1'b0".to_string(), "1'b0".to_string())
                    } else if never {
                        ("1'b1".to_string(), "1'b1".to_string())
                    } else if c.ty == "CY2_OBUF" {
                        (pin_net("DATAOUT", "OUT_CONST", "DATA_INVERT", &mut invert)?, "1'b1".to_string())
                    } else {
                        (
                            pin_net("DATAOUT", "OUT_CONST", "DATA_INVERT", &mut invert)?,
                            pin_net("OE", "OE_CONST", "OE_INVERT", &mut invert)?,
                        )
                    };
                    let io = vid(&format!("{p}~I"));
                    writeln!(v, "cycloneii_io {io}(")?;
                    writeln!(v, "  .datain({d}),\n  .oe({oe}),")?;
                    for (port, pin) in [("outclk", "OUTCLK"), ("inclk", "INCLK")] {
                        if c.conns.contains_key(pin) {
                            writeln!(v, "  .{port}({}),", net(c, pin, "1'b0"))?;
                        }
                    }
                    for (port, pin) in [("combout", "COMBOUT"), ("regout", "REGOUT")] {
                        if c.conns.contains_key(pin) {
                            writeln!(v, "  .{port}({}),", net(c, pin, ""))?;
                        }
                    }
                    writeln!(v, "  .padio({}));", vid(p))?;
                    let mode = match (c.ty.as_str(), bidir) {
                        (_, true) => "bidir",
                        ("CY2_IBUF", _) => "input",
                        _ => "output",
                    };
                    writeln!(v, "defparam {io}.operation_mode = \"{mode}\";")?;
                    for (r, used) in [("input", in_reg), ("output", out_reg), ("oe", oe_reg)] {
                        let m = if used.is_some() { "register" } else { "none" };
                        writeln!(v, "defparam {io}.{r}_register_mode = \"{m}\";")?;
                        if let Some(power_up) = used {
                            writeln!(v, "defparam {io}.{r}_power_up = \"{power_up}\";")?;
                        }
                    }
                    if never {
                        writeln!(v, "defparam {io}.open_drain_output = \"true\";")?;
                    }
                }
            }
        }
    }
    for (sig, w) in &inverted {
        writeln!(v, "wire {};\nassign {} = ~{};", vid(w), vid(w), vid(sig))?;
    }
    writeln!(v, "endmodule")?;

    // Routing.
    let ioe_out_x = |x: u32, y: u32, z: u32| -> u32 {
        pin_at(io, x, y, z).map_or(x, |p| x + u32::from(p.ioe.block == BLOCK_COL && z < 2))
    };
    // quartus_map names a VQM atom after its (first) output net, and
    // the rcf uses those names for signals and sinks alike.
    let mut rcf_name: HashMap<&str, String> = HashMap::new();
    for (name, c) in &r.cells {
        // Pins are named after their pad (an I/O atom's `padio`).
        let out = ["COMBOUT", "COUT", "REGOUT", "OUTCLK"].iter().find_map(|p| c.conns.get(*p).and_then(|id| driver.get(id)));
        let out = if matches!(c.bel, Bel::Ioe { .. }) { None } else { out };
        // A RAM atom is named after its lowest output bit (q1).
        let out = out.or_else(|| {
            (0..m4k::COLS).find_map(|k| c.conns.get(&format!("DO{k}")).and_then(|id| driver.get(id))).filter(|_| c.ty == "CY2_M4K")
        });
        // The serial-flash block is named after its data0out, the JTAG
        // block after its TDO (quartus_map's names, as the rcf of a
        // Quartus-fitted design shows).
        let out = out.or_else(|| c.conns.get("DATA0OUT").and_then(|id| driver.get(id)).filter(|_| c.ty == "CY2_ASMI"));
        let out = out.cloned().or_else(|| (c.bel == Bel::Jtag).then(|| format!("{}~TDO", atom[name.as_str()])));
        rcf_name.insert(name, out.unwrap_or_else(|| atom[name.as_str()].clone()));
    }
    let atom = rcf_name;
    let perm = perms(&r.nets);
    // Every pad-to-core path at delay setting 6, the model's (and
    // Quartus' untimed) choice; with a timing constraint the fitter
    // otherwise picks per destination.
    for (name, c) in &r.cells {
        let Bel::Ioe { z, .. } = c.bel else { continue };
        if !matches!(c.ty.as_str(), "CY2_IBUF" | "CY2_IOBUF") || z >= 8 {
            continue;
        }
        let Some(id) = c.conns.get("COMBOUT") else { continue };
        let mut sinks: Vec<&String> = loads.get(id).into_iter().flatten().map(|(n, _)| &atom[*n]).collect();
        sinks.sort();
        sinks.dedup();
        for s in sinks {
            writeln!(qsf, "set_instance_assignment -name PAD_TO_CORE_DELAY 6 -from {} -to {s}", atom[name.as_str()])?;
        }
    }
    let mut rcf = String::new();
    writeln!(rcf, "section global_data {{\n\trcf_written_by = \"cram quartus-export\";\n\tdevice = EP2C35F672C6;\n}}\n")?;
    for (&id, n) in &r.nets {
        let Some(sig) = driver.get(&id) else { continue };
        // Tree over resources; sinks are (cell, port) dests.
        let mut kids: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut dests: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut has_parent: std::collections::BTreeSet<String> = Default::default();
        let mut order: Vec<String> = Vec::new();
        let seen = |r: &String, order: &mut Vec<String>| {
            if !order.contains(r) {
                order.push(r.clone());
            }
        };
        for p in &n.pips {
            let rs = resource(dev, p.src, ioe_out_x);
            let rd = resource(dev, p.dst, ioe_out_x);
            match (rs, rd) {
                (Some(s), Some(d)) => {
                    seen(&s, &mut order);
                    seen(&d, &mut order);
                    kids.entry(s).or_default().push(d.clone());
                    has_parent.insert(d.clone());
                    if let Wire::Ioe { x, y, z, pin } = p.dst {
                        let c = cell_at.get(&Bel::Ioe { x, y, z }).context("output pin cell")?;
                        let port = if pin == IoePin::Oe { "OE" } else { "DATAIN" };
                        dests.entry(d).or_default().push(format!("dest = ( {}, {port} );", atom[*c]));
                    } else if let Wire::PllIn { x, y, input } = p.dst {
                        let c = cell_at.get(&Bel::Pll { x, y }).with_context(|| format!("{}: PLL pin without its cell", n.name))?;
                        dests.entry(d).or_default().push(format!("dest = ( {}, {} );", atom[*c], input.pin()));
                    } else if let Wire::Node(dn) = p.dst {
                        // A PLL's clock input is the PLL_CLKIN element itself.
                        if dev.element_enum(dn) == 329 {
                            let (x, y) = dev.xy(dn);
                            let c = cell_at
                                .get(&Bel::Pll { x: x as u32, y: y as u32 })
                                .with_context(|| format!("{}: PLL clock input without its cell", n.name))?;
                            dests.entry(d).or_default().push(format!("dest = ( {}, INCLK[0] );", atom[*c]));
                        }
                    } else if let Wire::Hard(h) = p.dst {
                        let (bel, port) = h.bel_pin();
                        let b = if bel == "JTAG" { Bel::Jtag } else { Bel::Asmi };
                        let c = cell_at.get(&b).with_context(|| format!("{}: {bel} pin without its cell", n.name))?;
                        dests.entry(d).or_default().push(format!("dest = ( {}, {port} );", atom[*c]));
                    }
                }
                // Into the net's first resource from its driver's pin.
                (None, Some(d)) => seen(&d, &mut order),
                (Some(s), None) => {
                    seen(&s, &mut order);
                    let dest = match p.dst {
                        Wire::Le { x, y, k, pin: LePin::Input(ph) } => {
                            let lut = cell_at.get(&Bel::Lut { x, y, k });
                            let names = ["DATAA", "DATAB", "DATAC", "DATAD"];
                            let phys = names[ph];
                            let lg: Vec<usize> = (0..4).filter(|&q| perm.get(&(x, y, k)).is_some_and(|m| m[q] == Some(ph))).collect();
                            let sdata_ff = cell_at
                                .get(&Bel::Ff { x, y, k })
                                .filter(|ff| ph == 2 && r.cells[**ff].conns.get("SDATA") == Some(&id));
                            if let Some(ff) = sdata_ff {
                                dests.entry(s.clone()).or_default().push(format!("dest = ( {}, SDATA ), route_port = SDATA;", atom[*ff]));
                            }
                            ensure!(!lg.is_empty() || sdata_ff.is_some(), "{}: LE input {phys} at X{x}Y{y} LE{k} feeds nothing", n.name);
                            let d: Vec<String> = match lut {
                                _ if lg.is_empty() => vec![],
                                Some(c) if feeder.contains_key(*c) => {
                                    let ff = cell_at.get(&Bel::Ff { x, y, k }).context("feeder's register")?;
                                    vec![format!("dest = ( {}, DATAIN ), route_port = {phys};", atom[*ff])]
                                }
                                Some(c) => lg.iter().map(|&q| format!("dest = ( {}, {} ), route_port = {phys};", atom[*c], names[q])).collect(),
                                None => bail!("{}: LUT input routed at X{x}Y{y} LE{k} without a LUT", n.name),
                            };
                            for x in d {
                                dests.entry(s.clone()).or_default().push(x);
                            }
                            continue;
                        }
                        Wire::Le { x, y, k, pin: pin @ (LePin::Clk | LePin::Ena | LePin::Aclr | LePin::Sclr) } => {
                            let c = cell_at.get(&Bel::Ff { x, y, k }).context("FF at LE control")?;
                            let port = match pin {
                                LePin::Clk => "CLK",
                                LePin::Ena => "ENA",
                                LePin::Aclr => "ACLR",
                                _ => "SCLR",
                            };
                            format!("dest = ( {}, {port} );", atom[*c])
                        }
                        Wire::ClkIn { g } => {
                            let c = cell_at.get(&Bel::ClkCtrl { g }).context("clock control")?;
                            format!("dest = ( {}, INCLK[0] );", atom[*c])
                        }
                        Wire::M4k { x, y, pin } => {
                            let c = cell_at.get(&Bel::M4k { x, y }).context("M4K at its pin")?;
                            let port = match pin {
                                M4kPin::In(i) => {
                                    let ports = atom_ports(&M4kShape::of(&r.cells[*c])?)?;
                                    ports.get(&i).with_context(|| format!("{}: M4K pin {i:?} is no atom port", n.name))?.clone()
                                }
                                M4kPin::Fast(f) => match m4k::CONTROL_PINS[f as usize].0 {
                                    "WE" => "PORTAWE".to_string(),
                                    p => p.to_string(),
                                },
                                M4kPin::Ctl(_) => bail!("{}: M4K control input as a sink", n.name),
                            };
                            format!("dest = ( {}, {port} );", atom[*c])
                        }
                        Wire::Ioe { x, y, z, pin: pin @ (IoePin::InClk | IoePin::OutClk) } => {
                            let c = cell_at.get(&Bel::Ioe { x, y, z }).context("pin cell at its clock")?;
                            format!("dest = ( {}, {} );", atom[*c], if pin == IoePin::InClk { "INCLK" } else { "OUTCLK" })
                        }
                        w => bail!("{}: sink {w:?}", n.name),
                    };
                    dests.entry(s).or_default().push(dest);
                }
                (None, None) => {}
            }
        }
        if order.is_empty() {
            continue;
        }
        writeln!(rcf, "signal_name = {sig} {{")?;
        fn emit(r: &str, kids: &BTreeMap<String, Vec<String>>, dests: &BTreeMap<String, Vec<String>>, out: &mut String) {
            let mut items: Vec<Result<&String, &String>> = Vec::new();
            items.extend(dests.get(r).into_iter().flatten().map(Err));
            items.extend(kids.get(r).into_iter().flatten().map(Ok));
            let label = format!("Label_{r}");
            if items.len() > 1 {
                let _ = writeln!(out, "\tlabel = {label}, {r};");
            } else {
                let _ = writeln!(out, "\t{r};");
            }
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    let _ = writeln!(out, "\n\tbranch_point = {label};");
                }
                match it {
                    Err(d) => {
                        let _ = writeln!(out, "\t{d}");
                    }
                    Ok(c) => emit(c, kids, dests, out),
                }
            }
        }
        let roots: Vec<&String> = order.iter().filter(|r| !has_parent.contains(*r)).collect();
        for (i, r) in roots.iter().enumerate() {
            if i > 0 {
                writeln!(rcf)?;
            }
            emit(r, &kids, &dests, &mut rcf);
        }
        writeln!(rcf, "}}")?;
    }
    Ok(QuartusExport { vqm: v, qsf, rcf })
}
