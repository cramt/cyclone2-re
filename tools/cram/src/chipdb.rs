//! nextpnr himbaechel chip database, as a flat text description that
//! `nextpnr/cyclone2/gen/arch_gen.py` turns into a .bba with himbaechel's own
//! generator. `cram db` commits it as `db/EP2C35F672C6/chipdb.txt.xz`, so
//! building the chip database takes Python only.
//!
//! Layout. Quartus tile (x, y) becomes two nextpnr tiles:
//!
//! - (2x, y), routing: one wire `G<node>` per routing-graph element homed
//!   there, plus an alias `G<src>` for every foreign element that drives
//!   one of them. Every graph edge is a pip in its destination's tile.
//!   Those tiles are all different, so each gets its own tile type.
//! - (2x+1, y), logic: LAB tiles share one tile type `LAB`, whose wires
//!   alias the LAB's local lines (`LI<l>`), LE output buffers (`LEB<i>`)
//!   and row-clock buffers (`BCB<b>`); the LE input muxes, LE output
//!   muxes and clock selects are its pips. I/O and clock-control tiles
//!   alias graph elements by their `G<node>` name.
//!
//! The names are the contract with the uarch (`nextpnr/cyclone2/*.cc`) and
//! the nextpnr frontend (`nextpnr.rs`), which turns routed pips back into
//! graph edges and block settings.

use anyhow::{ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;

use chipsmith_cyclone2::bitstream::asmdb::arch;
use chipsmith_cyclone2::bitstream::device::{element_name, ROUTER_ID_BIT};
use chipsmith_cyclone2::bitstream::io::{BLOCK_CLK_N, BLOCK_CLK_P, BLOCK_COL};
use chipsmith_cyclone2::bitstream::m4k::{self, CONTROL_PINS as M4K_CONTROL_PINS};
use chipsmith_cyclone2::bitstream::mult::{self as bmult, arch as mult_arch};
use chipsmith_cyclone2::mult::{Operand, Pin as MultPin};
use chipsmith_cyclone2::bitstream::phys::Db;

/// The JTAG block's outputs into the core: (rcf resource kind, atom port,
/// routing element enum), matched on designs that route each one.
pub const JTAG_OUTPUTS: [(&str, &str, i16); 8] = [
    ("JTAG_TMSUTAP_PIN", "tmsutap", 361),
    ("JTAG_TCKUTAP_PIN", "tckutap", 362),
    ("JTAG_TDIUTAP_PIN", "tdiutap", 363),
    ("JTAG_SHIFTUSER_PIN", "shiftuser", 364),
    ("JTAG_CLKDRUSER_PIN", "clkdruser", 365),
    ("JTAG_UPDATEUSER_PIN", "updateuser", 366),
    ("JTAG_RUNIDLEUSER_PIN", "runidleuser", 367),
    ("JTAG_USR1USER_PIN", "usr1user", 368),
];
pub const ASMI_DATA0OUT: i16 = 474;

pub const WIDTH: u32 = 2 * 66;
pub const HEIGHT: u32 = 37;
/// LE input mux sources: 38 local lines, then the 16 LEs' own outputs.
pub const LAB_LINES: usize = 38;
pub const LES: usize = 16;
pub const LE_INPUTS: [&str; 4] = ["A", "B", "C", "D"];

#[derive(Default)]
struct TileType {
    wires: BTreeMap<String, String>,
    pips: BTreeSet<(String, String)>,
    /// name, type, z, pins (pin, wire, is_output)
    bels: Vec<(String, String, u32, Vec<(String, String, bool)>)>,
}

impl TileType {
    fn wire(&mut self, name: &str, ty: &str) {
        self.wires.entry(name.to_string()).or_insert_with(|| ty.to_string());
    }
    fn pip(&mut self, src: &str, dst: &str) {
        self.pips.insert((src.to_string(), dst.to_string()));
    }
    fn write(&self, name: &str, w: &mut impl Write) -> Result<()> {
        writeln!(w, "tt {name}")?;
        for (n, t) in &self.wires {
            writeln!(w, "w {n} {t}")?;
        }
        for (n, t, z, pins) in &self.bels {
            writeln!(w, "b {n} {t} {z}")?;
            for (p, wire, out) in pins {
                writeln!(w, "bp {p} {wire} {}", if *out { 'O' } else { 'I' })?;
            }
        }
        for (s, d) in &self.pips {
            writeln!(w, "p {s} {d}")?;
        }
        writeln!(w, "end")?;
        Ok(())
    }
}

/// The shared LAB tile type: 16 LEs (LUT at z = 2k, register at 2k+1),
/// their input, output and clock muxes.
fn lab_tile(db: &Db) -> Result<TileType> {
    let mut t = TileType::default();
    for l in 0..LAB_LINES {
        t.wire(&format!("LI{l}"), "LOCAL_INTERCONNECT");
    }
    for i in 0..2 * LES {
        t.wire(&format!("LEB{i}"), "LE_BUFFER");
    }
    for b in 0..6 {
        t.wire(&format!("BCB{b}"), "BLK_CLK_BUF");
    }
    for k in 0..=LES {
        t.wire(&format!("CY{k}"), "CARRY");
    }
    // LAB control lines (rcf LAB_CONTROL_MUX), fed like an LE input.
    let pia: Vec<_> = (0..4).map(|k| db.lab.feature(arch::LAB_PIA_TO_INPUT, k)).collect::<Result<_>>()?;
    for (k, f) in pia.iter().enumerate() {
        t.wire(&format!("CTRL{k}"), "LAB_CONTROL");
        let aux = f.aux.as_ref().context("LAB_PIA_TO_INPUT is not a source mux")?;
        for (s, &a) in aux.iter().enumerate() {
            if a < 0 || f.source_bits(0, s).is_err() {
                continue;
            }
            let src = if s < LAB_LINES { format!("LI{s}") } else { format!("LL{}", s - LAB_LINES) };
            t.pip(&src, &format!("CTRL{k}"));
        }
    }
    // Which control lines (sources 0-3) and row-clock buffers (4-9) each
    // register control can take, from the fast-mux source maps.
    let reachable = |arch_type: i32, group: i32| -> Result<Vec<usize>> {
        let f = db.lab.feature(arch_type, group)?;
        let aux = f.aux.as_ref().context("fast mux without source map")?;
        Ok((0..aux.len()).filter(|&s| aux[s] >= 0).collect())
    };
    let mut ena: Vec<usize> = reachable(arch::LAB_CE_FAST_MUX, 0)?;
    ena.extend(reachable(arch::LAB_CE_FAST_MUX, 1)?);
    // The model drives a LAB-line clear through clear mux 0 only.
    let aclr = reachable(arch::LAB_CLEAR_FAST_MUX, 0)?;
    let sclr = reachable(arch::LAB_SYNC_CLR_FAST_MUX, 0)?;
    let inputs: Vec<_> = (0..4).map(|p| db.lab.feature(arch::LE_LUT_INPUT, p)).collect::<Result<_>>()?;
    for k in 0..LES {
        t.wire(&format!("LL{k}"), "LOCAL_LINE");
        for p in LE_INPUTS {
            t.wire(&format!("LE{k}.{p}"), "LE_INPUT");
            t.wire(&format!("LE{k}.I{p}"), "LUT_INPUT");
        }
        for o in ["COMB", "Q", "CLK", "ENA", "ACLR", "SCLR"] {
            t.wire(&format!("LE{k}.{o}"), "LE_INTERNAL");
        }
    }
    for k in 0..LES {
        let le = format!("LE{k}");
        let mut lut = vec![("CIN".into(), format!("CY{k}"), false), ("COUT".into(), format!("CY{}", k + 1), true)];
        lut.push(("COMBOUT".into(), format!("{le}.COMB"), true));
        for (p, f) in LE_INPUTS.iter().zip(&inputs) {
            // The LUT's logical inputs take any physical input (the LUT
            // mask is permuted to match, as the fitter does); carry LEs
            // keep theirs (the uarch's checkPipAvail).
            lut.push((format!("DATA{p}"), format!("{le}.I{p}"), false));
            for q in LE_INPUTS {
                t.pip(&format!("{le}.{q}"), &format!("{le}.I{p}"));
            }
            let aux = f.aux.as_ref().context("LE_LUT_INPUT is not a source mux")?;
            ensure!(aux.len() == LAB_LINES + LES, "LE_LUT_INPUT has {} sources", aux.len());
            for (s, &a) in aux.iter().enumerate() {
                if a < 0 || f.source_bits(k, s).is_err() {
                    continue;
                }
                let src = if s < LAB_LINES { format!("LI{s}") } else { format!("LL{}", s - LAB_LINES) };
                t.pip(&src, &format!("{le}.{p}"));
            }
        }
        t.bels.push((format!("{le}.LUT"), "LCELL_COMB".into(), 2 * k as u32, lut));
        // The register's data input is its own LUT's output (no other path
        // is modelled), so the two pins share a wire.
        let ff = vec![
            ("DATAIN".into(), format!("{le}.COMB"), false),
            ("CLK".into(), format!("{le}.CLK"), false),
            ("ENA".into(), format!("{le}.ENA"), false),
            ("ACLR".into(), format!("{le}.ACLR"), false),
            ("SCLR".into(), format!("{le}.SCLR"), false),
            // Synchronous load data: the LE's physical input C.
            ("SDATA".into(), format!("{le}.C"), false),
            ("REGOUT".into(), format!("{le}.Q"), true),
        ];
        for (pin, srcs) in [("ENA", &ena), ("ACLR", &aclr), ("SCLR", &sclr)] {
            for &s in srcs.iter() {
                let src = if s < 4 { format!("CTRL{s}") } else if s < 10 { format!("BCB{}", s - 4) } else { continue };
                t.pip(&src, &format!("{le}.{pin}"));
            }
        }
        t.bels.push((format!("{le}.FF"), "LCELL_FF".into(), 2 * k as u32 + 1, ff));
        // Output select (LE_REG_OUT_YGR): each of the LE's three outputs
        // takes the LUT or the register.
        for o in [format!("LEB{}", 2 * k), format!("LEB{}", 2 * k + 1), format!("LL{k}")] {
            t.pip(&format!("{le}.COMB"), &o);
            t.pip(&format!("{le}.Q"), &o);
        }
        // LAB clock: any row-clock buffer through the LAB clock muxes.
        for b in 0..6 {
            t.pip(&format!("BCB{b}"), &format!("{le}.CLK"));
        }
    }
    Ok(t)
}

/// LOCAL_INTERCONNECT lines at an M4K tile (0-81; the input muxes reach
/// 0-80).
pub const M4K_LINES: usize = 82;
pub const M4K_OUTPUTS: usize = 36;

/// The M4K's physical input pins: bel pin name and the asmdb source mux
/// (arch type, group) that selects its LAB line. `DATA_INPUT` 18-35 and
/// `BYTE_ENABLE_INPUT` 2-3 are the same muxes as `_B` 0-17 / 0-1 (m4k.rs).
pub fn m4k_input_pins() -> Vec<(String, i32, i32)> {
    m4k::Input::all()
        .into_iter()
        .map(|i| {
            let (t, g) = i.group();
            (i.pin_name(), t, g)
        })
        .collect()
}

/// The shared M4K tile type: one bel `M4K` (type `M4K`) whose input pins
/// sit behind the block's source muxes over its LAB lines (`LI<l>`), whose
/// control pins take a control input (`M4K.CTL<k>`, itself fed from a line
/// by `RAM_PIA_TO_INPUT_YGR` k) or a row clock (`BCB<b>`), and whose
/// outputs are the M4K_BUFFER elements (`MB<k>`, column k of the read
/// word).
fn m4k_tile(db: &Db) -> Result<TileType> {
    let mut t = TileType::default();
    for l in 0..M4K_LINES {
        t.wire(&format!("LI{l}"), "LOCAL_INTERCONNECT");
    }
    for b in 0..6 {
        t.wire(&format!("BCB{b}"), "BLK_CLK_BUF");
    }
    for k in 0..M4K_OUTPUTS {
        t.wire(&format!("MB{k}"), "M4K_BUFFER");
    }
    let lines = |at: i32, grp: i32| -> Result<Vec<usize>> {
        let f = db.m4k.feature(at, grp)?;
        let aux = f.aux.as_ref().context("M4K input is not a source mux")?;
        Ok((0..aux.len().min(M4K_LINES)).filter(|&s| aux[s] >= 0 && f.source_bits(0, s).is_ok()).collect())
    };
    let mut pins = Vec::new();
    for k in 0..8 {
        let w = format!("M4K.CTL{k}");
        t.wire(&w, "M4K_CONTROL");
        for s in lines(arch::RAM_PIA_TO_INPUT_YGR, k)? {
            t.pip(&format!("LI{s}"), &w);
        }
    }
    for (pin, at, grp) in m4k_input_pins() {
        let w = format!("M4K.{pin}");
        t.wire(&w, "M4K_INPUT");
        for s in lines(at, grp)? {
            t.pip(&format!("LI{s}"), &w);
        }
        pins.push((pin, w, false));
    }
    for (pin, at, grp) in M4K_CONTROL_PINS {
        let w = format!("M4K.{pin}");
        t.wire(&w, "M4K_INPUT");
        let f = db.m4k.feature(at, grp)?;
        let aux = f.aux.as_ref().context("M4K fast mux without source map")?;
        for (s, &a) in aux.iter().enumerate() {
            match s {
                _ if a < 0 => {}
                0..=7 => t.pip(&format!("M4K.CTL{s}"), &w),
                8..=13 => t.pip(&format!("BCB{}", s - 8), &w),
                // Source 14 of the clear and write-enable muxes: not a
                // control input or row clock; what it is is unknown.
                _ => {}
            }
        }
        pins.push((pin.to_string(), w, false));
    }
    for k in 0..M4K_OUTPUTS {
        pins.push((format!("DO{k}"), format!("MB{k}"), true));
    }
    t.bels.push(("M4K".into(), "M4K".into(), 0, pins));
    Ok(t)
}

/// The source mux (arch type, group) behind a multiplier input pin fed
/// straight from a LAB line; the clock and clear have a fast mux instead.
fn mult_mux(pin: MultPin) -> (i32, i32) {
    match pin {
        MultPin::A(k) => (mult_arch::MAC_ROW_DATA_INPUT, k as i32),
        MultPin::B(k) => (mult_arch::MAC_ROW_DATA_INPUT, (bmult::DATA / 2) as i32 + k as i32),
        MultPin::Sign(Operand::A) => (mult_arch::MAC_PIA_TO_SIGN, 0),
        MultPin::Sign(Operand::B) => (mult_arch::MAC_PIA_TO_SIGN, 1),
        MultPin::Ena => (mult_arch::MAC_PIA_TO_CE, 0),
        MultPin::Clk | MultPin::Aclr => unreachable!("behind a fast mux"),
        MultPin::Out(_) => unreachable!("outputs have no mux"),
    }
}

/// The shared multiplier tile type: one bel `MULT` (type `MULT`) whose data,
/// sign and enable inputs sit behind the block's source muxes over its 46
/// LAB lines (`LI<l>`), whose clock and clear take a control input
/// (`MULT.CTL0`, `MULT.CTL2`, each fed from a line) or a row clock
/// (`BCB<b>`) through their fast mux (bitstream `mult.rs`), and whose
/// outputs are the DSP_BUFFER elements (`DB<k>`, product bit k).
fn mult_tile(db: &Db) -> Result<TileType> {
    let mut t = TileType::default();
    for l in 0..bmult::LINES {
        t.wire(&format!("LI{l}"), "LOCAL_INTERCONNECT");
    }
    for b in 0..6 {
        t.wire(&format!("BCB{b}"), "BLK_CLK_BUF");
    }
    for k in 0..bmult::DATA {
        t.wire(&format!("DB{k}"), "DSP_BUFFER");
    }
    for (k, at) in [(0, mult_arch::MAC_PIA_TO_CLK), (2, mult_arch::MAC_PIA_TO_CLR)] {
        let w = format!("MULT.CTL{k}");
        t.wire(&w, "MULT_CONTROL");
        for l in db.mult.sources(at, 0)? {
            t.pip(&format!("LI{}", l.index()), &w);
        }
    }
    let mut pins = Vec::new();
    for pin in MultPin::all() {
        if let MultPin::Out(k) = pin {
            pins.push((pin.to_string(), format!("DB{k}"), true));
            continue;
        }
        let w = format!("MULT.{pin}");
        t.wire(&w, "MULT_INPUT");
        match pin {
            MultPin::Clk | MultPin::Aclr => {
                let k = if pin == MultPin::Clk { 0 } else { 2 };
                t.pip(&format!("MULT.CTL{k}"), &w);
                for b in 0..6 {
                    t.pip(&format!("BCB{b}"), &w);
                }
            }
            _ => {
                let (at, grp) = mult_mux(pin);
                for l in db.mult.sources(at, grp)? {
                    t.pip(&format!("LI{}", l.index()), &w);
                }
            }
        }
        pins.push((pin.to_string(), w, false));
    }
    t.bels.push(("MULT".into(), "MULT".into(), 0, pins));
    Ok(t)
}

pub fn export(db: &Db, w: &mut impl Write) -> Result<()> {
    let dev = &db.dev;
    let r = &dev.routing;
    let n = r.elements.len();
    let tmpl = |i: usize| &r.templates[r.elements[i].template as usize];
    let home = |i: usize| (r.elements[i].x as u32, r.elements[i].y as u32);
    let find = |x: u32, y: u32, e: i16, z: i16, idx: i16| -> Result<usize> {
        let v: Vec<_> = dev.at(x as i16, y as i16, z, idx).iter().filter(|&&nd| dev.element_enum(nd) == e).collect();
        ensure!(v.len() == 1, "{} {} at X{x} Y{y} S{z} I{idx}", v.len(), element_name(e));
        Ok(v[0].0 as usize)
    };

    // LOCAL_LINE elements have no edges; the LAB tile models them.
    let mut fanin: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut has_edge = vec![false; n];
    for src in 0..n {
        if tmpl(src).element_enum == 43 {
            continue;
        }
        for gid in r.fanout(src) {
            if gid & ROUTER_ID_BIT == 0 {
                continue;
            }
            let d = (gid & !ROUTER_ID_BIT) as usize;
            if d < n {
                fanin[d].push(src as u32);
                has_edge[src] = true;
                has_edge[d] = true;
            }
        }
    }
    for f in &mut fanin {
        f.sort();
        f.dedup();
    }

    // Routing tiles.
    let mut members: Vec<Vec<(u32, u32, String)>> = vec![Vec::new(); n];
    let mut rtiles: BTreeMap<(u32, u32), TileType> = BTreeMap::new();
    for i in (0..n).filter(|&i| has_edge[i]) {
        let (x, y) = home(i);
        members[i].push((2 * x, y, format!("G{i}")));
        rtiles.entry((x, y)).or_default().wire(&format!("G{i}"), &element_name(tmpl(i).element_enum));
    }
    for d in (0..n).filter(|&i| has_edge[i]) {
        let (x, y) = home(d);
        for &s in &fanin[d] {
            let s = s as usize;
            let t = rtiles.get_mut(&(x, y)).unwrap();
            if home(s) != (x, y) && !t.wires.contains_key(&format!("G{s}")) {
                t.wire(&format!("G{s}"), &element_name(tmpl(s).element_enum));
                members[s].push((2 * x, y, format!("G{s}")));
            }
            t.pip(&format!("G{s}"), &format!("G{d}"));
        }
    }

    // Logic tiles.
    let labs = db.lab.lab_locs();
    let lab_set: BTreeSet<(u32, u32)> = labs.iter().copied().collect();
    for &(x, y) in &labs {
        let mut alias = |node: usize, name: String| members[node].push((2 * x + 1, y, name));
        for l in 0..LAB_LINES {
            alias(find(x, y, 276, 0, l as i16)?, format!("LI{l}"));
        }
        for i in 0..2 * LES {
            alias(find(x, y, 45, 0, i as i16)?, format!("LEB{i}"));
        }
        for b in 0..6 {
            alias(find(x, y, 560, 0, b)?, format!("BCB{b}"));
        }
    }
    let m4ks = db.m4k.locs();
    let m4k_set: BTreeSet<(u32, u32)> = m4ks.iter().copied().collect();
    for &(x, y) in &m4ks {
        let mut alias = |node: usize, name: String| members[node].push((2 * x + 1, y, name));
        for l in 0..M4K_LINES {
            alias(find(x, y, 276, 0, l as i16)?, format!("LI{l}"));
        }
        for b in 0..6 {
            alias(find(x, y, 560, 0, b)?, format!("BCB{b}"));
        }
        for k in 0..M4K_OUTPUTS {
            alias(find(x, y, 278, 0, k as i16)?, format!("MB{k}"));
        }
    }
    let mult_rows = db.mult.rows();
    let mult_set: BTreeSet<(u32, u32)> = mult_rows.iter().map(|&y| (bmult::X, y)).collect();
    for &(x, y) in &mult_set {
        let mut alias = |node: usize, name: String| members[node].push((2 * x + 1, y, name));
        for l in 0..bmult::LINES {
            alias(find(x, y, 276, 0, l as i16)?, format!("LI{l}"));
        }
        for b in 0..6 {
            alias(find(x, y, 560, 0, b)?, format!("BCB{b}"));
        }
        for k in 0..bmult::DATA {
            alias(find(x, y, 280, 0, k as i16)?, format!("DB{k}"));
        }
    }
    let mut ltiles: BTreeMap<(u32, u32), TileType> = BTreeMap::new();
    let alias_g = |ltiles: &mut BTreeMap<(u32, u32), TileType>, members: &mut Vec<Vec<(u32, u32, String)>>, (x, y): (u32, u32), node: usize| {
        let name = format!("G{node}");
        let t = ltiles.entry((x, y)).or_default();
        if !t.wires.contains_key(&name) {
            t.wire(&name, &element_name(tmpl(node).element_enum));
            members[node].push((2 * x + 1, y, name.clone()));
        }
        name
    };
    let mut pads = Vec::new();
    let mut pins: Vec<_> = db.io.pins.values().collect();
    pins.sort_by(|a, b| a.name.cmp(&b.name));
    for p in pins {
        let (px, py, pz) = (p.xyz.0 as u32, p.xyz.1 as u32, p.xyz.2 as u32);
        let clock = [BLOCK_CLK_P, BLOCK_CLK_N].contains(&p.ioe.block);
        let z = pz + if clock { 8 } else { 0 };
        let bel = format!("IOE{z}");
        let (din, dout, oe) = (format!("{bel}.COMBOUT"), format!("{bel}.DATAOUT"), format!("{bel}.OE"));
        {
            let t = ltiles.entry((px, py)).or_default();
            t.wire(&din, "IOE_INTERNAL");
            t.wire(&dout, "IOE_INTERNAL");
            ensure!(!t.bels.iter().any(|b| b.0 == bel), "two pads at X{px} Y{py} z{z}");
            let mut pins = vec![("DATAOUT".into(), dout.clone(), false), ("COMBOUT".into(), din.clone(), true)];
            if !clock {
                t.wire(&oe, "IOE_INTERNAL");
                pins.push(("OE".into(), oe.clone(), false));
                // The I/O registers: the input register's clock and output,
                // and the clock the output and OE registers share
                // (chipsmith `bitstream::io`).
                for (pin, out) in [("INCLK", false), ("OUTCLK", false), ("REGOUT", true)] {
                    let w = format!("{bel}.{pin}");
                    t.wire(&w, "IOE_INTERNAL");
                    pins.push((pin.into(), w, out));
                }
            }
            t.bels.push((bel.clone(), "IOE".into(), z, pins));
        }
        if clock {
            let g = alias_g(&mut ltiles, &mut members, (px, py), find(px, py, 289, pz as i16, 0)?);
            ltiles.get_mut(&(px, py)).unwrap().pip(&din, &g);
        } else {
            // The input register reaches the core on the same two
            // pad-to-core paths as the pad itself (rcf: a registered input
            // starts at IO_DATAIN).
            let regout = format!("{bel}.REGOUT");
            for i in 0..2 {
                let g = alias_g(&mut ltiles, &mut members, (px, py), find(px, py, 301, pz as i16, i)?);
                let t = ltiles.get_mut(&(px, py)).unwrap();
                t.pip(&din, &g);
                t.pip(&regout, &g);
            }
            // I/O register clocks come from a row-clock buffer of the
            // IOE's tile, a column IOE's from the tile to its right
            // (observed: every INCLK/OUTCLK route of the reg and oereg fuzz
            // designs, 2,061 on all sides, ends at BLK_CLK_BUF X+1 on a
            // column IOE, X on a row IOE).
            let cx = px + u32::from(p.ioe.block == BLOCK_COL);
            for b in 0..6 {
                let Ok(node) = find(cx, py, 560, 0, b) else { continue };
                let g = alias_g(&mut ltiles, &mut members, (px, py), node);
                let t = ltiles.get_mut(&(px, py)).unwrap();
                t.pip(&g, &format!("{bel}.INCLK"));
                t.pip(&g, &format!("{bel}.OUTCLK"));
            }
            // Column IOEs 0 and 1 take their output and output enable from
            // the local lines of the tile to the right (observed on every
            // column output and tri-state pin of the fuzz designs: rcf
            // IO_DATAOUT and IO_OE_PIN at X+1).
            let ox = px + u32::from(p.ioe.block == BLOCK_COL && pz < 2);
            for (sink, lines) in [(&dout, db.io.data_sources(p)), (&oe, db.io.oe_sources(p))] {
                for l in lines {
                    let g = alias_g(&mut ltiles, &mut members, (px, py), find(ox, py, 276, 0, l as i16)?);
                    ltiles.get_mut(&(px, py)).unwrap().pip(&g, sink);
                }
            }
        }
        pads.push((p.name.clone(), format!("X{}Y{py}", 2 * px + 1), bel, p.bank));
    }
    // Global clock controls: a CLKBUF_OUT that drives GLOBAL_CLK_H G is
    // clock control G; its input select picks a CLKBUF_IN of the same
    // tile and z by index.
    for o in (0..n).filter(|&i| tmpl(i).element_enum == 515) {
        let g_nodes: Vec<usize> = r
            .fanout(o)
            .filter(|gid| gid & ROUTER_ID_BIT != 0)
            .map(|gid| (gid & !ROUTER_ID_BIT) as usize)
            .filter(|&d| d < n && tmpl(d).element_enum == 292)
            .collect();
        let [gn] = g_nodes[..] else { continue };
        let g = tmpl(gn).index as u32;
        let (x, y) = home(o);
        let z = tmpl(o).z;
        let out = alias_g(&mut ltiles, &mut members, (x, y), o);
        let inclk = format!("CLKCTRL{g}.INCLK");
        let mut srcs = Vec::new();
        for idx in 0..8 {
            if let Ok(i) = find(x, y, 513, z, idx) {
                srcs.push(alias_g(&mut ltiles, &mut members, (x, y), i));
            }
        }
        let t = ltiles.get_mut(&(x, y)).unwrap();
        t.wire(&inclk, "CLKCTRL_INTERNAL");
        for s in srcs {
            t.pip(&s, &inclk);
        }
        t.bels.push((format!("CLKCTRL{g}"), "CLKCTRL".into(), 32 + g, vec![
            ("INCLK".into(), inclk, false),
            ("OUTCLK".into(), out, true),
        ]));
    }
    // The PLLs (chipsmith `bitstream::pll`): bel `PLL` in each PLL's tile.
    // INCLK is the PLL's clock input element (PLL_CLKIN 0, fed from the
    // dedicated clock pins of its side through CLKPIN_TO_PLLCLKIN_MUX), CLKk
    // its counter k's output (PLL_BUFFER k, into the clock controls), LOCKED
    // the lock output (PLL_LOCKED_PIN) onto the routing; ARESET and PFDENA
    // sit behind source muxes over the tile's LAB lines.
    {
        use chipsmith_cyclone2::bitstream::pll::{Control, Site};
        for site in Site::ALL {
            let (x, y) = site.xy();
            let mut pins = vec![("INCLK".to_string(), alias_g(&mut ltiles, &mut members, (x, y), find(x, y, 329, 0, 0)?), false)];
            for k in 0..3 {
                pins.push((format!("CLK{k}"), alias_g(&mut ltiles, &mut members, (x, y), find(x, y, 69, 0, k)?), true));
            }
            pins.push(("LOCKED".into(), alias_g(&mut ltiles, &mut members, (x, y), find(x, y, 347, 0, 0)?), true));
            for c in Control::ALL {
                let w = format!("PLL.{}", c.pin());
                let mut srcs = Vec::new();
                for l in db.pll.control_sources(site, c)? {
                    srcs.push(alias_g(&mut ltiles, &mut members, (x, y), find(x, y, 276, 0, l as i16)?));
                }
                let t = ltiles.entry((x, y)).or_default();
                t.wire(&w, "PLL_INPUT");
                for s in srcs {
                    t.pip(&s, &w);
                }
                pins.push((c.pin().to_string(), w, false));
            }
            ltiles.entry((x, y)).or_default().bels.push(("PLL".into(), "PLL".into(), 48, pins));
        }
    }
    // The JTAG and serial-flash blocks (chipsmith `bitstream::hard`): one bel each, inputs
    // behind source muxes over the tile's local lines, outputs on their
    // routing elements.
    {
        use chipsmith_cyclone2::bitstream::hard::{HardInput, ASMI_XY, JTAG_XY};
        let mut pins: BTreeMap<&str, Vec<(String, String, bool)>> = BTreeMap::new();
        for input in HardInput::ALL {
            let (bel, pin) = input.bel_pin();
            let (x, y) = input.xy();
            let w = format!("{bel}.{pin}");
            let mut srcs = Vec::new();
            for l in db.hard.sources(input) {
                srcs.push(alias_g(&mut ltiles, &mut members, (x, y), find(x, y, 276, 0, l.index() as i16)?));
            }
            let t = ltiles.entry((x, y)).or_default();
            t.wire(&w, "HARD_INPUT");
            for s in srcs {
                t.pip(&s, &w);
            }
            pins.entry(bel).or_default().push((pin.to_string(), w, false));
        }
        let element = |e: i16| -> Result<usize> {
            let v: Vec<usize> = (0..n).filter(|&i| tmpl(i).element_enum == e).collect();
            ensure!(v.len() == 1, "{} elements {}", v.len(), element_name(e));
            Ok(v[0])
        };
        for (_, port, e) in JTAG_OUTPUTS {
            let g = alias_g(&mut ltiles, &mut members, JTAG_XY, element(e)?);
            pins.entry("JTAG").or_default().push((port.to_uppercase(), g, true));
        }
        let g = alias_g(&mut ltiles, &mut members, ASMI_XY, element(ASMI_DATA0OUT)?);
        pins.entry("ASMI").or_default().push(("DATA0OUT".into(), g, true));
        for (bel, xy) in [("JTAG", JTAG_XY), ("ASMI", ASMI_XY)] {
            let p = pins.remove(bel).unwrap_or_default();
            ltiles.entry(xy).or_default().bels.push((bel.into(), bel.into(), 0, p));
        }
    }
    for xy in ltiles.keys() {
        ensure!(!lab_set.contains(xy), "LAB tile X{} Y{} also holds I/O or clock control", xy.0, xy.1);
        ensure!(!m4k_set.contains(xy), "M4K tile X{} Y{} also holds I/O or clock control", xy.0, xy.1);
        ensure!(!mult_set.contains(xy), "multiplier tile X{} Y{} also holds I/O or clock control", xy.0, xy.1);
    }

    // Write.
    writeln!(w, "chip {WIDTH} {HEIGHT}")?;
    TileType::default().write("NULL", w)?;
    lab_tile(db)?.write("LAB", w)?;
    m4k_tile(db)?.write("M4K", w)?;
    mult_tile(db)?.write("MULT", w)?;
    for ((x, y), t) in &rtiles {
        t.write(&format!("R{x}_{y}"), w)?;
    }
    for ((x, y), t) in &ltiles {
        t.write(&format!("L{x}_{y}"), w)?;
    }
    for ty in 0..HEIGHT {
        for tx in 0..WIDTH {
            let (x, y) = (tx / 2, ty);
            let name = if tx % 2 == 0 {
                rtiles.contains_key(&(x, y)).then(|| format!("R{x}_{y}"))
            } else if lab_set.contains(&(x, y)) {
                Some("LAB".into())
            } else if m4k_set.contains(&(x, y)) {
                Some("M4K".into())
            } else if mult_set.contains(&(x, y)) {
                Some("MULT".into())
            } else {
                ltiles.contains_key(&(x, y)).then(|| format!("L{x}_{y}"))
            };
            writeln!(w, "t {tx} {ty} {}", name.as_deref().unwrap_or("NULL"))?;
        }
    }
    for m in members.iter().filter(|m| m.len() > 1) {
        write!(w, "node")?;
        for (x, y, name) in m {
            write!(w, " {x} {y} {name}")?;
        }
        writeln!(w)?;
    }
    // Carry chains run from LE 15 of a LAB into LE 0 of the LAB below
    // (counters: X36_Y2 N30 -> X36_Y1 N0).
    for &(x, y) in &labs {
        if lab_set.contains(&(x, y - 1)) {
            writeln!(w, "node {} {y} CY{LES} {} {} CY0", 2 * x + 1, 2 * x + 1, y - 1)?;
        }
    }
    writeln!(w, "pkg F672")?;
    for (pin, tile, bel, bank) in pads {
        writeln!(w, "pad {pin} {tile} {bel} {bank}")?;
    }
    let mut stats: HashMap<&str, usize> = HashMap::new();
    stats.insert("routing tiles", rtiles.len());
    stats.insert("routing pips", rtiles.values().map(|t| t.pips.len()).sum());
    stats.insert("routing wires", rtiles.values().map(|t| t.wires.len()).sum());
    stats.insert("max wires per tile", rtiles.values().chain(ltiles.values()).map(|t| t.wires.len()).max().unwrap_or(0));
    stats.insert("LABs", labs.len());
    stats.insert("I/O and clock tiles", ltiles.len());
    eprintln!("chipdb: {stats:?}");
    Ok(())
}
