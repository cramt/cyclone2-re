//! nextpnr frontend: a routed himbaechel JSON (`nextpnr-himbaechel --write`,
//! uarch cyclone2) -> physical netlist, and the same design as Quartus
//! constraints (an atom netlist, location assignments and an rcf) so the
//! oracle can assemble exactly this placement and routing.
//!
//! Wire and bel names follow `chipdb` (which documents them).

use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;

use ddb::device::{Device, Node};

use crate::chipdb::element_name;
use crate::io::{IoDb, OutData, PinUse, Role, Standard, UnusedPins, BLOCK_CLK_N, BLOCK_CLK_P, BLOCK_COL};
use crate::logic::{clock_slots, Aclr, ClkCtrl, Ctl, Domain, GlobalClock, LabConfig, LeConfig, LeSource, Logic, Lut, Reg, Sload};
use crate::phys::{PhysDesign, Pip, RoutedNet};

/// A nextpnr wire, by what it is in the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Wire {
    /// A routing-graph element.
    Node(Node),
    /// LOCAL_LINE k of LAB (x, y): LE k's direct output into its own LAB.
    LocalLine { x: u32, y: u32, k: u32 },
    /// A pin of LE k (LUT input A..D, COMB, Q, CLK).
    Le { x: u32, y: u32, k: u32, pin: LePin },
    /// Output data / pad-to-core of the I/O element at logic tile (x, y),
    /// bel z.
    Ioe { x: u32, y: u32, z: u32, pin: IoePin },
    ClkIn { g: u32 },
    /// LAB control line k (rcf LAB_CONTROL_MUX).
    Ctrl { x: u32, y: u32, k: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LePin {
    /// Physical LE input A..D (fed by the LAB's lines).
    Input(usize),
    /// Logical LUT input DATAA..DATAD (a cell pin), fed by any physical one.
    Logical(usize),
    Comb,
    Q,
    Clk,
    Ena,
    Aclr,
    Sclr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum IoePin {
    DataOut,
    CombOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Bel {
    Lut { x: u32, y: u32, k: u32 },
    Ff { x: u32, y: u32, k: u32 },
    Ioe { x: u32, y: u32, z: u32 },
    ClkCtrl { g: u32 },
}

#[derive(Debug)]
struct Cell {
    ty: String,
    bel: Bel,
    params: HashMap<String, String>,
    attrs: HashMap<String, String>,
    /// port -> net id (only connected ports)
    conns: HashMap<String, u64>,
}

#[derive(Debug)]
struct PipUse {
    src: Wire,
    dst: Wire,
}

#[derive(Debug, Default)]
struct Net {
    name: String,
    pips: Vec<PipUse>,
}

pub struct Routed {
    cells: BTreeMap<String, Cell>,
    nets: BTreeMap<u64, Net>,
}

fn tile_xy(t: &str) -> Result<(u32, u32)> {
    let r = t.strip_prefix('X').with_context(|| format!("tile {t}"))?;
    let (x, y) = r.split_once('Y').with_context(|| format!("tile {t}"))?;
    Ok((x.parse()?, y.parse()?))
}

fn num(s: &str, prefix: &str) -> Option<u32> {
    s.strip_prefix(prefix)?.parse().ok()
}

fn find(dev: &Device, x: u32, y: u32, e: i16, z: i16, idx: u32) -> Result<Node> {
    let v: Vec<Node> =
        dev.at(x as i16, y as i16, z, idx as i16).iter().copied().filter(|&n| dev.element_enum(n) == e).collect();
    match v[..] {
        [n] => Ok(n),
        _ => bail!("{} {} at X{x} Y{y} S{z} I{idx}", v.len(), element_name(e)),
    }
}

fn wire(dev: &Device, tx: u32, ty: u32, name: &str) -> Result<Wire> {
    if let Some(n) = num(name, "G") {
        return Ok(Wire::Node(Node(n)));
    }
    let (x, y) = (tx / 2, ty);
    if let Some(l) = num(name, "LI") {
        return Ok(Wire::Node(find(dev, x, y, 276, 0, l)?));
    }
    if let Some(i) = num(name, "LEB") {
        return Ok(Wire::Node(find(dev, x, y, 45, 0, i)?));
    }
    if let Some(b) = num(name, "BCB") {
        return Ok(Wire::Node(find(dev, x, y, 560, 0, b)?));
    }
    if let Some(k) = num(name, "LL") {
        return Ok(Wire::LocalLine { x, y, k });
    }
    if let Some(k) = num(name, "CTRL") {
        return Ok(Wire::Ctrl { x, y, k });
    }
    if let Some((bel, pin)) = name.split_once('.') {
        if let Some(k) = num(bel, "LE") {
            let pin = match pin {
                "IA" => LePin::Logical(0),
                "IB" => LePin::Logical(1),
                "IC" => LePin::Logical(2),
                "ID" => LePin::Logical(3),
                "A" => LePin::Input(0),
                "B" => LePin::Input(1),
                "C" => LePin::Input(2),
                "D" => LePin::Input(3),
                "COMB" => LePin::Comb,
                "Q" => LePin::Q,
                "CLK" => LePin::Clk,
                "ENA" => LePin::Ena,
                "ACLR" => LePin::Aclr,
                "SCLR" => LePin::Sclr,
                _ => bail!("wire {name}"),
            };
            return Ok(Wire::Le { x, y, k, pin });
        }
        if let Some(z) = num(bel, "IOE") {
            let pin = match pin {
                "DATAOUT" => IoePin::DataOut,
                "COMBOUT" => IoePin::CombOut,
                _ => bail!("wire {name}"),
            };
            return Ok(Wire::Ioe { x, y, z, pin });
        }
        if let Some(g) = num(bel, "CLKCTRL") {
            return Ok(Wire::ClkIn { g });
        }
    }
    bail!("unknown wire {name} in tile X{tx}Y{ty}")
}

fn bel(tx: u32, ty: u32, name: &str) -> Result<Bel> {
    let (x, y) = (tx / 2, ty);
    if let Some(k) = name.strip_suffix(".LUT").and_then(|b| num(b, "LE")) {
        return Ok(Bel::Lut { x, y, k });
    }
    if let Some(k) = name.strip_suffix(".FF").and_then(|b| num(b, "LE")) {
        return Ok(Bel::Ff { x, y, k });
    }
    if let Some(z) = num(name, "IOE") {
        return Ok(Bel::Ioe { x, y, z });
    }
    if let Some(g) = num(name, "CLKCTRL") {
        return Ok(Bel::ClkCtrl { g });
    }
    bail!("unknown bel {name}")
}

fn strs(v: &Value) -> HashMap<String, String> {
    v.as_object()
        .map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
        .unwrap_or_default()
}

pub fn read(dev: &Device, path: &Path) -> Result<Routed> {
    let j: Value = serde_json::from_str(&std::fs::read_to_string(path).with_context(|| path.display().to_string())?)?;
    let m = j["modules"]["top"].as_object().context("no module top")?;
    let mut cells = BTreeMap::new();
    for (name, c) in m["cells"].as_object().context("cells")? {
        let attrs = strs(&c["attributes"]);
        let b = attrs.get("NEXTPNR_BEL").with_context(|| format!("{name} is not placed"))?;
        let (t, bn) = b.split_once('/').context("bel name")?;
        let (tx, ty) = tile_xy(t)?;
        let mut conns = HashMap::new();
        for (p, bits) in c["connections"].as_object().context("connections")? {
            if let Some(id) = bits.as_array().and_then(|a| a.first()).and_then(Value::as_u64) {
                conns.insert(p.clone(), id);
            }
        }
        cells.insert(
            name.clone(),
            Cell { ty: c["type"].as_str().unwrap_or("").to_string(), bel: bel(tx, ty, bn)?, params: strs(&c["parameters"]), attrs, conns },
        );
    }
    let mut nets: BTreeMap<u64, Net> = BTreeMap::new();
    for (name, n) in m["netnames"].as_object().context("netnames")? {
        let Some(id) = n["bits"].as_array().and_then(|a| a.first()).and_then(Value::as_u64) else { continue };
        let net = nets.entry(id).or_default();
        let routing = n["attributes"]["ROUTING"].as_str().unwrap_or("");
        if net.name.is_empty() || !routing.is_empty() {
            net.name = name.clone();
        }
        let parts: Vec<&str> = routing.split(';').collect();
        for t in parts.chunks(3) {
            let [_, pip, _] = t else { continue };
            if pip.is_empty() {
                continue;
            }
            let mut f = pip.split('/');
            let (Some(tile), Some(dst), Some(src)) = (f.next(), f.next(), f.next()) else { bail!("pip {pip}") };
            let (tx, ty) = tile_xy(tile)?;
            net.pips.push(PipUse { src: wire(dev, tx, ty, src)?, dst: wire(dev, tx, ty, dst)? });
        }
    }
    Ok(Routed { cells, nets })
}

/// Logical -> physical LUT input of each LE (from the permutation pips).
type Perm = HashMap<(u32, u32, u32), [Option<usize>; 4]>;

fn perms(nets: &BTreeMap<u64, Net>) -> Perm {
    let mut out: Perm = HashMap::new();
    for p in nets.values().flat_map(|n| &n.pips) {
        if let (Wire::Le { pin: LePin::Input(ph), .. }, Wire::Le { x, y, k, pin: LePin::Logical(lg) }) = (p.src, p.dst) {
            out.entry((x, y, k)).or_default()[lg] = Some(ph);
        }
    }
    out
}

/// The truth table over physical inputs. Unrouted logical inputs keep their
/// own position: the mask doesn't depend on them, except in carry LEs,
/// where C is the carry in and D picks sum or carry and both stay put.
fn physical_mask(logical: u16, perm: [Option<usize>; 4]) -> u16 {
    let mut out = 0u16;
    for idx_p in 0..16 {
        let mut idx_l = 0;
        for q in 0..4 {
            idx_l |= (idx_p >> perm[q].unwrap_or(q) & 1) << q;
        }
        out |= (logical >> idx_l & 1) << idx_p;
    }
    out
}

fn flag(c: &Cell, p: &str) -> bool {
    c.params.get(p).is_some_and(|v| v.contains('1'))
}

fn lut_mask(c: &Cell) -> Result<u16> {
    let s = c.params.get("LUT_MASK").context("LUT_MASK")?;
    Ok(u16::from_str_radix(s, 2)?)
}

/// Package pin at an I/O bel.
fn pin_at(io: &IoDb, x: u32, y: u32, z: u32) -> Result<&crate::io::Pin> {
    let clock = z >= 8;
    io.pins
        .values()
        .find(|p| {
            (p.xyz.0 as u32, p.xyz.1 as u32, p.xyz.2 as u32) == (x, y, z % 8)
                && clock == [BLOCK_CLK_P, BLOCK_CLK_N].contains(&p.ioe.block)
        })
        .with_context(|| format!("no pin at X{x} Y{y} IOE{z}"))
}

fn le_at(les: &mut BTreeMap<(u32, u32, u32), LeConfig>, x: u32, y: u32, k: u32) -> &mut LeConfig {
    les.entry((x, y, k)).or_insert_with(|| LeConfig {
        x,
        y,
        le: k,
        lut: None,
        inputs: [None; 4],
        carry_in: false,
        reg_feedback: false,
        reg: None,
        comb_out: [false; 3],
    })
}

impl Routed {
    pub fn to_phys(&self, dev: &Device, io: &IoDb) -> Result<PhysDesign> {
        let mut les: BTreeMap<(u32, u32, u32), LeConfig> = BTreeMap::new();
        let mut clk_buf: HashMap<(u32, u32, u32), u8> = HashMap::new();
        let mut ena: HashMap<(u32, u32, u32), u8> = HashMap::new();
        let mut aclr: HashMap<(u32, u32, u32), Aclr> = HashMap::new();
        let mut sclr: HashMap<(u32, u32, u32), u8> = HashMap::new();
        let mut pia: HashMap<(u32, u32, u32), LeSource> = HashMap::new();
        let mut clk_in: HashMap<u32, u8> = HashMap::new();
        let mut data_line: HashMap<(u32, u32, u32), u32> = HashMap::new();
        let mut datain: HashMap<(u32, u32, u32), [bool; 2]> = HashMap::new();
        let mut nets = Vec::new();
        for n in self.nets.values() {
            let mut pips = Vec::new();
            for p in &n.pips {
                match (p.src, p.dst) {
                    (Wire::Node(s), Wire::Node(d)) => {
                        pips.push(Pip::new(dev, s, d).with_context(|| format!("{}: no edge {s:?} -> {d:?}", n.name))?)
                    }
                    (Wire::Le { pin: LePin::Input(_), .. }, Wire::Le { pin: LePin::Logical(_), .. }) => {}
                    (src, dst @ (Wire::Le { pin: LePin::Input(_), .. } | Wire::Ctrl { .. })) => {
                        let s = match src {
                            Wire::Node(s) => {
                                let t = &dev.routing.templates[dev.routing.elements[s.0 as usize].template as usize];
                                ensure!(t.element_enum == 276, "{}: LE input from {}", n.name, element_name(t.element_enum));
                                LeSource::Line(t.index as u8)
                            }
                            Wire::LocalLine { k: j, .. } => LeSource::Feedback(j as u8),
                            w => bail!("{}: LE input from {w:?}", n.name),
                        };
                        match dst {
                            Wire::Le { x, y, k, pin: LePin::Input(i) } => le_at(&mut les, x, y, k).inputs[i] = Some(s),
                            Wire::Ctrl { x, y, k } => {
                                pia.insert((x, y, k), s);
                            }
                            _ => unreachable!(),
                        }
                    }
                    (Wire::Ctrl { k: line, .. }, Wire::Le { x, y, k, pin }) => {
                        let line = line as u8;
                        match pin {
                            LePin::Ena => ena.insert((x, y, k), line),
                            LePin::Sclr => sclr.insert((x, y, k), line),
                            LePin::Aclr => aclr.insert((x, y, k), Aclr::Pia(line)).map(|_| 0),
                            p => bail!("{}: control line into LE pin {p:?}", n.name),
                        };
                    }
                    (Wire::Node(s), Wire::Le { x, y, k, pin: LePin::Aclr }) => {
                        let t = &dev.routing.templates[dev.routing.elements[s.0 as usize].template as usize];
                        aclr.insert((x, y, k), Aclr::Global(t.index as u8));
                    }
                    (Wire::Le { x, y, k, pin }, dst) if matches!(pin, LePin::Comb | LePin::Q) => {
                        let o = match dst {
                            Wire::Node(d) => {
                                let t = &dev.routing.templates[dev.routing.elements[d.0 as usize].template as usize];
                                ensure!(t.element_enum == 45, "{}: LE output into {}", n.name, element_name(t.element_enum));
                                t.index as u32 - 2 * k
                            }
                            Wire::LocalLine { .. } => 2,
                            w => bail!("{}: LE output into {w:?}", n.name),
                        };
                        le_at(&mut les, x, y, k).comb_out[o as usize] = pin == LePin::Comb;
                    }
                    (Wire::Node(s), Wire::Le { x, y, k, pin: LePin::Clk }) => {
                        let t = &dev.routing.templates[dev.routing.elements[s.0 as usize].template as usize];
                        clk_buf.insert((x, y, k), t.index as u8);
                    }
                    (Wire::Node(s), Wire::ClkIn { g }) => {
                        let t = &dev.routing.templates[dev.routing.elements[s.0 as usize].template as usize];
                        clk_in.insert(g, t.index as u8);
                    }
                    (Wire::Node(s), Wire::Ioe { x, y, z, pin: IoePin::DataOut }) => {
                        let t = &dev.routing.templates[dev.routing.elements[s.0 as usize].template as usize];
                        data_line.insert((x, y, z), t.index as u32);
                    }
                    (Wire::Ioe { x, y, z, pin: IoePin::CombOut }, Wire::Node(d)) => {
                        let t = &dev.routing.templates[dev.routing.elements[d.0 as usize].template as usize];
                        if t.element_enum == 301 {
                            datain.entry((x, y, z)).or_default()[t.index as usize] = true;
                        }
                    }
                    (s, d) => bail!("{}: unexpected pip {s:?} -> {d:?}", n.name),
                }
            }
            nets.push(RoutedNet { name: n.name.clone(), pips });
        }

        let perm = perms(&self.nets);
        let mut clkctrls = Vec::new();
        let mut pins = Vec::new();
        let mut ff_domain: HashMap<(u32, u32, u32), Domain> = HashMap::new();
        let sdata: std::collections::HashSet<(u32, u32, u32)> = self
            .cells
            .values()
            .filter_map(|c| match c.bel {
                Bel::Ff { x, y, k } if c.conns.contains_key("SDATA") => Some((x, y, k)),
                _ => None,
            })
            .collect();
        let sclr_inv: std::collections::HashSet<(u32, u32, u32)> = self
            .cells
            .values()
            .filter_map(|c| match c.bel {
                Bel::Ff { x, y, k } if flag(c, "SCLR_INV") => Some((x, y, k)),
                _ => None,
            })
            .collect();
        for (name, c) in &self.cells {
            match (c.ty.as_str(), c.bel) {
                ("LCELL_COMB", Bel::Lut { x, y, k }) => {
                    let l = le_at(&mut les, x, y, k);
                    l.lut = Some(Lut { mask: physical_mask(lut_mask(c)?, perm.get(&(x, y, k)).copied().unwrap_or_default()) });
                    l.carry_in = c.conns.contains_key("CIN");
                }
                ("LCELL_FF", Bel::Ff { x, y, k }) => {
                    let b = *clk_buf.get(&(x, y, k)).with_context(|| format!("{name}: clock not routed"))?;
                    let e = ena.get(&(x, y, k)).map(|&line| Ctl { line, inverted: flag(c, "ENA_INV") });
                    ff_domain.insert((x, y, k), (b, false, e));
                }
                ("CLKCTRL", Bel::ClkCtrl { g }) => {
                    let input = *clk_in.get(&g).with_context(|| format!("{name}: input not routed"))?;
                    clkctrls.push(ClkCtrl { g: g as u8, input });
                }
                ("CY2_IBUF" | "CY2_OBUF", Bel::Ioe { x, y, z }) => {
                    let p = pin_at(io, x, y, z)?;
                    let out = c.ty == "CY2_OBUF";
                    pins.push(PinUse {
                        pin: p.name.clone(),
                        port: c.attrs.get("PORT").cloned().unwrap_or_default(),
                        role: if out { Role::Out } else { Role::In },
                        standard: Standard::Lvttl33,
                        drive: None,
                        pull_up: false,
                        bus_hold: false,
                        data: match (c.params.get("OUT_CONST"), data_line.get(&(x, y, z))) {
                            (Some(v), _) => Some(OutData::Const(v.ends_with('1'))),
                            (None, Some(&line)) => Some(OutData::Line { line, inverted: c.params.get("DATA_INVERT").is_some_and(|v| v.ends_with('1')) }),
                            _ => None,
                        },
                        oe_line: None,
                        datain_used: datain.get(&(x, y, z)).copied().unwrap_or_default(),
                        in_reg: None,
                        out_reg: None,
                        oe_reg: false,
                    });
                }
                (t, b) => bail!("{name}: cell type {t} on {b:?}"),
            }
        }
        let mut labs: BTreeMap<(u32, u32), LabConfig> = BTreeMap::new();
        let mut by_lab: BTreeMap<(u32, u32), Vec<Domain>> = BTreeMap::new();
        for (&(x, y, _), d) in &ff_domain {
            by_lab.entry((x, y)).or_default().push(*d);
        }
        for (&xy, v) in &by_lab {
            let slots = clock_slots(v).with_context(|| format!("LAB {xy:?}"))?;
            let lab = labs.entry(xy).or_default();
            for (j, d) in slots.iter().enumerate() {
                if let Some(d) = d {
                    lab.clocks[j] = Some(GlobalClock { buf: d.0, inverted: d.1 });
                    lab.enables[j] = d.2;
                }
            }
            for (&(x, y, k), d) in ff_domain.iter().filter(|((x, y, _), _)| (*x, *y) == xy) {
                let clock = slots.iter().position(|s| *s == Some(*d)).unwrap() as u8;
                let a = aclr.get(&(x, y, k)).copied();
                let sc = sclr.get(&(x, y, k)).map(|&line| Ctl { line, inverted: sclr_inv.contains(&(x, y, k)) });
                if let Some(a) = a {
                    ensure!(lab.aclr.is_none_or(|o| o == a), "LAB {xy:?}: two clears");
                    lab.aclr = Some(a);
                }
                if let Some(c) = sc {
                    ensure!(lab.sync.sclr.is_none_or(|o| o == c), "LAB {xy:?}: two sync clears");
                    lab.sync.sclr = Some(c);
                }
                // A cleared register takes LAB clear line 0 (the model's
                // only one).
                // Registers on the sync load path load SDATA (input C) every
                // cycle through the LAB's sload, tied high.
                let sload = sdata.contains(&(x, y, k));
                if sload {
                    lab.sync.sload = Some(Sload::Const);
                }
                le_at(&mut les, x, y, k).reg = Some(Reg { clock, aclr: a.map(|_| 0), sync: sc.is_some() || sload });
            }
        }
        for l in les.values() {
            labs.entry((l.x, l.y)).or_default();
        }
        for (&(x, y, k), &src) in &pia {
            labs.entry((x, y)).or_default().pia[k as usize] = Some(src);
        }
        Ok(PhysDesign {
            logic: Logic { les: les.into_values().collect(), clkctrls, labs },
            pins,
            unused_pins: UnusedPins::InputTriStated,
            m4ks: vec![],
            nets,
        })
    }
}

/// `name[3]` -> ("name", Some(3)).
fn split_port(p: &str) -> (&str, Option<u32>) {
    if let Some((b, i)) = p.strip_suffix(']').and_then(|s| s.split_once('[')) {
        if let Ok(i) = i.parse() {
            return (b, Some(i));
        }
    }
    (p, None)
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
            Some(format!("{}:X{}Y{}S{}I{}", element_name(t.element_enum), e.x, e.y, t.z, t.index))
        }
        Wire::LocalLine { x, y, k } => Some(format!("LOCAL_LINE:X{x}Y{y}S0I{k}")),
        Wire::Ctrl { x, y, k } => Some(format!("LAB_CONTROL_MUX:X{x}Y{y}S0I{k}")),
        Wire::Ioe { x, y, z, pin: IoePin::DataOut } => Some(format!("IO_DATAOUT:X{}Y{y}S{}I0", ioe_out_x(x, y, z), z % 8)),
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

impl Routed {
    pub fn to_quartus(&self, dev: &Device, io: &IoDb) -> Result<QuartusExport> {
        // Atom names.
        let mut atom: HashMap<&str, String> = HashMap::new();
        let mut port_of: HashMap<&str, String> = HashMap::new();
        for (name, c) in &self.cells {
            let a = match c.bel {
                Bel::Lut { x, y, k } => format!("lc_x{x}_y{y}_n{}", 2 * k),
                Bel::Ff { x, y, k } => format!("ff_x{x}_y{y}_n{}", 2 * k + 1),
                Bel::ClkCtrl { g } => format!("clkctrl_g{g}"),
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
        for (name, c) in &self.cells {
            let a = &atom[name.as_str()];
            for (p, &id) in &c.conns {
                let sig = match (c.ty.as_str(), p.as_str()) {
                    ("LCELL_COMB", "COMBOUT") => format!("{a}_combout"),
                    ("LCELL_COMB", "COUT") => format!("{a}_cout"),
                    ("LCELL_FF", "REGOUT") => format!("{a}_regout"),
                    ("CLKCTRL", "OUTCLK") => format!("{a}_outclk"),
                    ("CY2_IBUF", "COMBOUT") => a.clone(),
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
        for (name, c) in &self.cells {
            for (p, &id) in &c.conns {
                if !["COMBOUT", "COUT", "REGOUT", "OUTCLK"].contains(&p.as_str()) && !(c.ty == "CY2_IBUF") {
                    loads.entry(id).or_default().push((name.as_str(), p.as_str()));
                }
            }
        }
        let cell_at: HashMap<Bel, &str> = self.cells.iter().map(|(n, c)| (c.bel, n.as_str())).collect();
        // LUT cell -> (source net, inverted)
        let mut feeder: HashMap<&str, (u64, bool)> = HashMap::new();
        for (name, c) in &self.cells {
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
        let mut ports: BTreeMap<String, (bool, Option<u32>)> = BTreeMap::new();
        for (name, p) in &port_of {
            let (b, i) = split_port(p);
            let e = ports.entry(b.to_string()).or_insert((self.cells[*name].ty == "CY2_OBUF", None));
            if let Some(i) = i {
                e.1 = Some(e.1.map_or(i, |m: u32| m.max(i)));
            }
        }
        let mut v = String::new();
        writeln!(v, "// Generated by `cram quartus-export` from a nextpnr routed design.")?;
        writeln!(v, "module top({});", ports.keys().map(|p| vid(p)).collect::<Vec<_>>().join(", "))?;
        for (p, (out, w)) in &ports {
            let r = w.map(|m| format!("[{m}:0] ")).unwrap_or_default();
            writeln!(v, "{} {r}{};", if *out { "output" } else { "input" }, vid(p))?;
        }
        let feeder_outs: std::collections::HashSet<&String> =
            feeder.keys().filter_map(|n| self.cells[*n].conns.get("COMBOUT").and_then(|id| driver.get(id))).collect();
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
        for (name, c) in &self.cells {
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
                    if c.ty == "CY2_OBUF" {
                        let inv = if c.params.get("DATA_INVERT").is_some_and(|v| v.ends_with('1')) { "~" } else { "" };
                        let d = match c.params.get("OUT_CONST") {
                            Some(k) => if k.ends_with('1') { "1'b1".to_string() } else { "1'b0".to_string() },
                            None => format!("{inv}{}", net(c, "DATAOUT", "1'b0")),
                        };
                        writeln!(v, "assign {} = {d};", vid(p))?;
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
        for (name, c) in &self.cells {
            let out = ["COMBOUT", "COUT", "REGOUT", "OUTCLK"].iter().find_map(|p| c.conns.get(*p).and_then(|id| driver.get(id)));
            rcf_name.insert(name, out.cloned().unwrap_or_else(|| atom[name.as_str()].clone()));
        }
        let atom = rcf_name;
        let perm = perms(&self.nets);
        // Every pad-to-core path at delay setting 6, the model's (and
        // Quartus' untimed) choice; with a timing constraint the fitter
        // otherwise picks per destination.
        for (name, c) in &self.cells {
            let Bel::Ioe { z, .. } = c.bel else { continue };
            if c.ty != "CY2_IBUF" || z >= 8 {
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
        for (&id, n) in &self.nets {
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
                        if let Wire::Ioe { x, y, z, .. } = p.dst {
                            let c = cell_at.get(&Bel::Ioe { x, y, z }).context("output pin cell")?;
                            dests.entry(d).or_default().push(format!("dest = ( {}, DATAIN );", atom[*c]));
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
                                    .filter(|ff| ph == 2 && self.cells[**ff].conns.get("SDATA") == Some(&id));
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
}
