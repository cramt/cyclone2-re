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
use chipsmith_cyclone2::m4k::{OutReg, Packing, Ports, Sel, Side, Width};
use chipsmith_cyclone2::bitstream::nextpnr::{flag, lut_mask, perms, pin_at, split_port, Bel, Cell, IoePin, LePin, M4kPin, M4kShape, PortDir, Routed, Wire};

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

/// One port of a RAM atom: which physical port it is, and what it does.
struct AtomPort {
    /// The atom's `a` or `b`.
    name: char,
    side: Side,
    width: Width,
    /// The atom's data width: the data bits the port carries.
    bits: usize,
    writes: bool,
    out_reg: Option<OutReg>,
    clock: Sel,
}

/// How Quartus sees a block: its operation mode and the atom's ports. A
/// read-only x36 is Quartus' "rom" atom, which it builds on port B (fuzz
/// r36), so the atom's port A is the block's port B.
fn atom_shape(shape: &M4kShape) -> (&'static str, Vec<AtomPort>) {
    let s = shape.shape;
    let port = |name: char, side: Side, writes: bool| -> AtomPort {
        let width = s.width(side).expect("a used port");
        let out_reg = s.read_ports().into_iter().find(|(x, _)| *x == side).and_then(|(_, r)| r.out_reg);
        // Bytes under byte enables sit in nine-column groups; Quartus puts
        // a VQM atom's bit k in column k whatever its byte size, so the
        // atom is the full physical width, the ninth columns unused.
        let bits = match s.packing() {
            Packing::Dense => s.data_bits(width),
            Packing::Bytes => width.bits(),
        };
        AtomPort { name, side, width, bits, writes, out_reg, clock: s.input_clock(side) }
    };
    match s.ports() {
        Ports::Single { .. } if shape.writes[0] => ("single_port", vec![port('a', Side::A, true)]),
        Ports::Single { .. } => ("rom", vec![port('a', Side::A, false)]),
        Ports::SimpleDual { .. } if shape.writes[0] => {
            ("dual_port", vec![port('a', Side::A, true), port('b', Side::B, false)])
        }
        Ports::SimpleDual { .. } => ("rom", vec![port('a', Side::B, false)]),
        Ports::TrueDual { .. } => {
            ("bidir_dual_port", vec![port('a', Side::A, shape.writes[0]), port('b', Side::B, shape.writes[1])])
        }
    }
}

/// Column of an atom port's bit k.
fn column(shape: &M4kShape, p: &AtomPort, k: usize) -> usize {
    match shape.shape.packing() {
        Packing::Dense => shape.shape.column(p.width, k),
        Packing::Bytes => k,
    }
}

/// Linear position of an atom port's word `word`, bit k.
fn linear(shape: &M4kShape, p: &AtomPort, word: usize, k: usize) -> usize {
    match shape.shape.packing() {
        Packing::Dense => shape.shape.linear(p.width, word, k),
        Packing::Bytes => p.width.linear(word, k),
    }
}

/// Data input pin of an atom port's bit k.
fn data_pin(shape: &M4kShape, p: &AtomPort, k: usize) -> m4k::Input {
    m4k::Input::Data((column(shape, p, k) + if p.side == Side::B { 18 } else { 0 }) as u8)
}

/// Read column (`DO<k>`) of an atom port's output bit k.
fn out_col(shape: &M4kShape, p: &AtomPort, k: usize) -> usize {
    let tdp = matches!(shape.shape.ports(), Ports::TrueDual { .. });
    column(shape, p, k) + if tdp && p.side == Side::B { 18 } else { 0 }
}

/// Quartus atom port (`PORTAADDR[i]`, ...) of each physical input.
fn atom_ports(shape: &M4kShape) -> Result<HashMap<m4k::Input, String>> {
    let mut out = HashMap::new();
    for p in atom_shape(shape).1 {
        let n = p.width.addr_bits();
        let up = p.name.to_ascii_uppercase();
        for i in 0..n {
            out.insert(m4k::Input::addr(p.side == Side::B, 12 - n + i)?, format!("PORT{up}ADDR[{i}]"));
        }
        if p.writes {
            for k in 0..p.bits {
                out.insert(data_pin(shape, &p, k), format!("PORT{up}DATAIN[{k}]"));
            }
            for j in 0..p.width.bits().div_ceil(9) {
                let be = if p.side == Side::B { 2 + j } else { j };
                out.insert(m4k::Input::ByteEna(be as u8), format!("PORT{up}BYTEENAMASKS[{j}]"));
            }
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
                ("CY2_IBUF", "COMBOUT") => a.clone(),
                ("CY2_IOBUF", "COMBOUT") => format!("{a}_combout"),
                ("CY2_M4K", o) if o.starts_with("DO") => format!("{a}_q{}", &o[2..]),
                ("CY2_JTAG", o) if HARD_OUTPUTS.contains(&o) => format!("{a}_{}", o.to_lowercase()),
                ("CY2_ASMI", "DATA0OUT") => format!("{a}_data0out"),
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
            let ram_out = (c.ty == "CY2_M4K" && p.starts_with("DO")) || HARD_OUTPUTS.contains(&p.as_str());
            if !["COMBOUT", "COUT", "REGOUT", "OUTCLK"].contains(&p.as_str()) && !(c.ty == "CY2_IBUF") && !ram_out {
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
                let (op, ports) = atom_shape(&shape);
                let pin = |inp: m4k::Input| net(c, &inp.pin_name(), "1'b0");
                let bus = |v: Vec<String>| format!("{{{}}}", v.into_iter().rev().collect::<Vec<_>>().join(", "));
                let sel = |s: Sel, what: &str| format!("\"{what}{}\"", s as u8);
                let mut conns: Vec<String> = Vec::new();
                conns.push(format!(".clk0({})", net(c, "CLK0", "1'b0")));
                for (pin_name, port) in [("CLK1", "clk1"), ("ENA0", "ena0"), ("ENA1", "ena1"), ("CLR0", "clr0"), ("CLR1", "clr1")] {
                    if c.conns.contains_key(pin_name) {
                        conns.push(format!(".{port}({})", net(c, pin_name, "1'b0")));
                    }
                }
                // A dual port must have port B's read enable driven
                // (error 14091), the other modes but true dual port must
                // not (14090).
                match op {
                    "dual_port" => conns.push(".portbrewe(1'b1)".to_string()),
                    "bidir_dual_port" => conns.push(format!(".portbrewe({})", net(c, "RE", "1'b0"))),
                    _ => {}
                }
                let mut params: Vec<(String, String)> = vec![
                    ("operation_mode".into(), format!("\"{op}\"")),
                    ("ram_block_type".into(), "\"M4K\"".into()),
                    ("logical_ram_name".into(), format!("\"{}\"", atom[name.as_str()])),
                    ("init_file_layout".into(), "\"port_a\"".into()),
                ];
                for p in &ports {
                    let n = p.width.addr_bits();
                    let depth = 1u32 << n;
                    let ab = p.side == Side::B;
                    let addr: Vec<String> = (0..n).map(|i| Ok(pin(m4k::Input::addr(ab, 12 - n + i)?))).collect::<Result<_>>()?;
                    conns.push(format!(".port{}addr({})", p.name, bus(addr)));
                    // A true dual port's port that never writes still has
                    // its write enable and data in (error 14091, 14107):
                    // tied off, as Quartus' own two-clock simple dual port
                    // has them.
                    if !p.writes && op == "bidir_dual_port" {
                        if p.name == 'a' {
                            conns.push(".portawe(1'b0)".to_string());
                        }
                        conns.push(format!(".port{}datain({}'b{})", p.name, p.bits, "1".repeat(p.bits)));
                    }
                    if p.writes {
                        let we = if p.side == Side::A { "WE" } else { "RE" };
                        if p.name == 'a' {
                            conns.push(format!(".portawe({})", net(c, we, "1'b0")));
                        }
                        let d: Vec<String> = (0..p.bits).map(|k| pin(data_pin(&shape, p, k))).collect();
                        conns.push(format!(".port{}datain({})", p.name, bus(d)));
                        let nbe = p.width.bits().div_ceil(9);
                        let be_pins: Vec<m4k::Input> = (0..nbe).map(|j| m4k::Input::ByteEna((j + if ab { 2 } else { 0 }) as u8)).collect();
                        if be_pins.iter().any(|b| c.conns.contains_key(&b.pin_name())) {
                            let be: Vec<String> = be_pins.iter().map(|&b| net(c, &b.pin_name(), "1'b1")).collect();
                            conns.push(format!(".port{}byteenamasks({})", p.name, bus(be)));
                            params.push((format!("port_{}_byte_enable_mask_width", p.name), nbe.to_string()));
                            params.push((format!("port_{}_byte_size", p.name), (p.bits / nbe).to_string()));
                        }
                    }
                    // A dual port's port A only writes.
                    if !(op == "dual_port" && p.name == 'a') {
                        let outs: Vec<String> = (0..p.bits)
                            .map(|k| match sig_of(c, &format!("DO{}", out_col(&shape, p, k))) {
                                Some(s) => vid(&s),
                                None => {
                                    let u = format!("{}_unused_{}q{k}", atom[name.as_str()], if p.name == 'a' { "" } else { "b" });
                                    writeln!(v, "wire {};", vid(&u)).unwrap();
                                    vid(&u)
                                }
                            })
                            .collect();
                        conns.push(format!(".port{}dataout({})", p.name, bus(outs)));
                    }
                    for (k, val) in [
                        ("address_width", n.to_string()),
                        ("data_width", p.bits.to_string()),
                        ("first_address", "0".into()),
                        ("last_address", (depth - 1).to_string()),
                        ("logical_ram_depth", depth.to_string()),
                        ("logical_ram_width", p.bits.to_string()),
                        ("first_bit_number", "0".into()),
                        ("data_out_clock", p.out_reg.map_or("\"none\"".into(), |o| sel(o.clock, "clock"))),
                        ("data_out_clear", p.out_reg.and_then(|o| o.clear).map_or("\"none\"".into(), |c| sel(c, "clear"))),
                    ] {
                        params.push((format!("port_{}_{k}", p.name), val));
                    }
                    if p.name == 'b' {
                        let clk = sel(p.clock, "clock");
                        params.push(("port_b_address_clock".into(), clk.clone()));
                        params.push(("port_b_read_enable_write_enable_clock".into(), clk.clone()));
                        if op == "bidir_dual_port" {
                            params.push(("port_b_data_in_clock".into(), clk.clone()));
                            if c.conns.keys().any(|k| k == "BE2" || k == "BE3") {
                                params.push(("port_b_byte_enable_clock".into(), clk));
                            }
                        } else {
                            params.push(("mixed_port_feed_through_mode".into(), "\"dont_care\"".into()));
                        }
                    }
                }
                writeln!(v, "cycloneii_ram_block {a}(\n  {});", conns.join(",\n  "))?;
                // Contents in the words of the atom's port A, 2,048 bits
                // per mem_init parameter (as Quartus writes them).
                let pa = &ports[0];
                let depth = 1usize << pa.width.addr_bits();
                let mut bits = Vec::with_capacity(depth * pa.bits);
                for word in 0..depth {
                    for k in 0..pa.bits {
                        bits.push(shape.content.get_linear(linear(&shape, pa, word, k)));
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
                    params.push((format!("mem_init{i}"), format!("{}'h{hex}", chunk.len())));
                }
                for (k, val) in params {
                    writeln!(v, "defparam {a}.{k} = {val};")?;
                }
                writeln!(qsf, "set_location_assignment M4K_X{x}_Y{y} -to {}", atom[name.as_str()])?;
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
                let cnst = |k: &String| if k.ends_with('1') { "1'b1".to_string() } else { "1'b0".to_string() };
                let bidir = r.port_dir(p)? == PortDir::InOut;
                if c.ty == "CY2_OBUF" && !bidir {
                    let inv = if flag(c, "DATA_INVERT") { "~" } else { "" };
                    let d = match c.params.get("OUT_CONST") {
                        Some(k) => cnst(k),
                        None => format!("{inv}{}", net(c, "DATAOUT", "1'b0")),
                    };
                    writeln!(v, "assign {} = {d};", vid(p))?;
                } else if c.ty != "CY2_IBUF" {
                    // The VQM reader takes no `?:`, so a tri-state pin
                    // is an I/O atom, the way Quartus' own VQM writer
                    // has it; so is any pin of an inout port (Quartus
                    // wants it in bidir mode). Inverted copies fold into the IOE
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
                    let (d, oe) = if never {
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
                    if c.conns.contains_key("COMBOUT") {
                        writeln!(v, "  .combout({}),", net(c, "COMBOUT", ""))?;
                    }
                    writeln!(v, "  .padio({}));", vid(p))?;
                    writeln!(v, "defparam {io}.operation_mode = \"{}\";", if bidir { "bidir" } else { "output" })?;
                    for r in ["input", "output", "oe"] {
                        writeln!(v, "defparam {io}.{r}_register_mode = \"none\";")?;
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
                                    "RE" => "PORTBREWE".to_string(),
                                    p => p.to_string(),
                                },
                                M4kPin::Ctl(_) => bail!("{}: M4K control input as a sink", n.name),
                            };
                            format!("dest = ( {}, {port} );", atom[*c])
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
