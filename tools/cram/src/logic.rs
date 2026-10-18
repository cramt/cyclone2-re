//! What a placed design asks of its LEs and LABs, in physical terms, and
//! the CRAM bits that follow.
//!
//! Inputs: the post-fit netlist (`vo`) for LUT masks, carry and register
//! wiring; the routing (`rcf`) for which LAB line feeds which LE input and
//! LAB control, and which global clock buffer feeds a LAB. Bit positions
//! all come from the asmdb (`lab`); the setting *values* below were read
//! off fuzz designs (specimens/lab, README "LE and LAB configuration") and
//! say so where they are not simply "the selected mux input".

use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeMap, HashMap};

use ddb::asmdb::arch;

use crate::lab::LabDb;
use crate::rcf::Signal;
use crate::vo::{self, Atom};

/// Number of LAB local interconnect lines; LE input mux sources 0..38 are
/// these, 38..54 the 16 LEs' own outputs (LOCAL_LINE), per the asmdb
/// LE_LUT_INPUT source maps (54 entries) and the counter design.
const LAB_LINES: u8 = 38;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeSource {
    /// LOCAL_INTERCONNECT line of the LAB.
    Line(u8),
    /// LOCAL_LINE: an LE of the same LAB feeding back directly.
    Feedback(u8),
}

impl LeSource {
    fn index(self) -> usize {
        match self {
            LeSource::Line(l) => l as usize,
            LeSource::Feedback(k) => LAB_LINES as usize + k as usize,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lut {
    /// Truth table over physical inputs, entry A + 2B + 4C + 8D.
    pub mask: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reg {
    /// LAB clock 0 or 1.
    pub clock: u8,
    /// LAB async clear line, if the register is cleared.
    pub aclr: Option<u8>,
    /// Takes the LAB's synchronous clear/load (one per-LE bit enables both).
    pub sync: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeConfig {
    pub x: u32,
    pub y: u32,
    pub le: u32,
    pub lut: Option<Lut>,
    /// Physical inputs A..D.
    pub inputs: [Option<LeSource>; 4],
    /// LUT input C comes from the carry chain (`sum_lutc_input = cin` with a
    /// real carry-in net).
    pub carry_in: bool,
    /// LUT input C is the LE's own register (no routing).
    pub reg_feedback: bool,
    pub reg: Option<Reg>,
    /// Output (LE_BUFFER n, LE_BUFFER n+1, LOCAL_LINE) driven by the LUT
    /// rather than the register.
    pub comb_out: [bool; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalClock {
    /// BLK_CLK_BUF index 0..5: which row clock the LAB taps.
    pub buf: u8,
    pub inverted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aclr {
    Pia(u8),
    Global(u8),
}

/// A LAB control signal arriving on control line `line`, possibly
/// inverted at the register port (the LAB has an invert bit for it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ctl {
    pub line: u8,
    pub inverted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sload {
    Pia(Ctl),
    /// sload tied high: registers load sdata (= their LE's datac) every
    /// cycle, how the fitter packs a register with an unrelated LUT.
    Const,
}

/// The LAB's synchronous clear and load; a LAB has one of each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sync {
    pub sclr: Option<Ctl>,
    pub sload: Option<Sload>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LabConfig {
    /// LAB_PIA_TO_INPUT k: what feeds LAB control line k (same source
    /// space as an LE input: local line or an LE's own output).
    pub pia: [Option<LeSource>; 4],
    pub clocks: [Option<GlobalClock>; 2],
    /// Clock enable of LAB clock j, as the control line carrying it.
    pub enables: [Option<Ctl>; 2],
    pub aclr: Option<Aclr>,
    pub sync: Sync,
}

/// A global clock control block in use: GCLK `g` driven from its clock
/// input `input` (CLKBUF_IN index: 0/1 dedicated clock pins, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClkCtrl {
    pub g: u8,
    pub input: u8,
}

#[derive(Debug, Default)]
pub struct Logic {
    pub les: Vec<LeConfig>,
    pub clkctrls: Vec<ClkCtrl>,
    pub labs: BTreeMap<(u32, u32), LabConfig>,
}

fn port_index(p: &str) -> Option<usize> {
    Some(match p {
        "DATAA" => 0,
        "DATAB" => 1,
        "DATAC" => 2,
        "DATAD" => 3,
        // A register's sdata enters through its LE's datac input mux.
        "SDATA" => 2,
        _ => return None,
    })
}

/// Names a cell or its output goes by: the rcf uses the node name (`q66`,
/// `cnt[8]~39`, or the carry-out net `co1` for a LUT whose only output
/// is its carry), the .vo the instance (`f67`) and output nets
/// (`cnt[8]~39_combout`).
fn names(a: &Atom) -> Vec<String> {
    let mut v = vec![a.name.clone()];
    for p in ["regout", "combout", "cout"] {
        if let Some(n) = a.port(p).filter(|n| !n.is_empty()) {
            v.push(n.to_string());
            if let Some(s) = n.strip_suffix("_combout") {
                v.push(s.to_string());
            }
        }
    }
    v
}

impl Logic {
    /// From the Quartus post-fit netlist and rcf. Also returns what the
    /// design uses that this model doesn't cover yet; those bits will be
    /// missing from the output.
    pub fn from_quartus(atoms: &[Atom], sigs: &[Signal]) -> Result<(Self, Vec<String>)> {
        let mut out = Logic::default();
        let mut unmodelled = Vec::new();
        // LE slots: comb and ff atoms by (x, y, le).
        let mut slots: BTreeMap<(u32, u32, u32), (Option<&Atom>, Option<&Atom>)> = BTreeMap::new();
        let mut cell: HashMap<String, (u32, u32, u32)> = HashMap::new();
        for a in atoms {
            let Some(loc) = vo::le_loc(&a.loc) else { continue };
            let s = slots.entry(loc).or_default();
            match a.kind.as_str() {
                "lcell_comb" => s.0 = Some(a),
                "lcell_ff" => s.1 = Some(a),
                k => bail!("{}: unexpected {k} at an LE location", a.name),
            }
            for n in names(a) {
                cell.insert(n, loc);
            }
        }

        // Routing facts per LE / LAB, from the rcf.
        let mut inputs: HashMap<(u32, u32, u32), [Option<LeSource>; 4]> = HashMap::new();
        let mut ctl: HashMap<(u32, u32, u32), HashMap<String, u8>> = HashMap::new(); // port -> pia
        let mut glob: HashMap<(u32, u32, u32), HashMap<String, u8>> = HashMap::new(); // port -> buf
        let mut outputs: HashMap<(u32, u32, u32), [Option<String>; 3]> = HashMap::new();
        let mut labs: BTreeMap<(u32, u32), LabConfig> = BTreeMap::new();
        for s in sigs {
            for (i, r) in s.resources.iter().enumerate() {
                let (x, y) = (r.x as u32, r.y as u32);
                match r.kind.as_str() {
                    "LE_BUFFER" => {
                        let o = outputs.entry((x, y, r.index as u32 / 2)).or_default();
                        o[r.index as usize % 2] = Some(s.name.clone());
                    }
                    "LOCAL_LINE" => {
                        outputs.entry((x, y, r.index as u32)).or_default()[2] = Some(s.name.clone());
                    }
                    "LAB_CONTROL_MUX" => {
                        let prev = s.parents[i].map(|j| &s.resources[j]);
                        let src = match prev.filter(|p| (p.x, p.y) == (r.x, r.y)) {
                            Some(p) if p.kind == "LOCAL_INTERCONNECT" => LeSource::Line(p.index as u8),
                            Some(p) if p.kind == "LOCAL_LINE" => LeSource::Feedback(p.index as u8),
                            _ => bail!("{}: LAB_CONTROL_MUX {r:?} not fed from its own LAB", s.name),
                        };
                        labs.entry((x, y)).or_default().pia[r.index as usize] = Some(src);
                    }
                    _ => {}
                }
            }
            for d in &s.dests {
                let Some(&loc) = cell.get(&d.cell) else { continue };
                let Some(r) = d.from.map(|i| &s.resources[i]) else { continue };
                match r.kind.as_str() {
                    "LOCAL_INTERCONNECT" | "LOCAL_LINE" => {
                        let Some(p) = port_index(d.physical_port()) else { continue };
                        let src = if r.kind == "LOCAL_LINE" {
                            LeSource::Feedback(r.index as u8)
                        } else {
                            LeSource::Line(r.index as u8)
                        };
                        inputs.entry(loc).or_default()[p] = Some(src);
                    }
                    "LAB_CONTROL_MUX" => {
                        ctl.entry(loc).or_default().insert(d.port.clone(), r.index as u8);
                    }
                    "BLK_CLK_BUF" => {
                        glob.entry(loc).or_default().insert(d.port.clone(), r.index as u8);
                    }
                    _ => {}
                }
            }
        }

        // LAB clocks: each register's domain, then `clock_slots`.
        let domain = |x: u32, y: u32, le: u32, ff: &Atom| -> Option<Domain> {
            let buf = *glob.get(&(x, y, le))?.get("CLK")?;
            let inv = ff.port("clk").is_some_and(|c| c.starts_with('!'));
            let ena = if ff.net("ena").is_some() {
                ctl.get(&(x, y, le)).and_then(|k| k.get("ENA")).map(|&line| Ctl { line, inverted: inverted(ff, "ena") })
            } else {
                None
            };
            Some((buf, inv, ena))
        };
        let mut lab_domains: BTreeMap<(u32, u32), Vec<Domain>> = BTreeMap::new();
        for (&(x, y, le), &(_, ff)) in &slots {
            let Some(ff) = ff else { continue };
            let Some(d) = domain(x, y, le, ff) else {
                unmodelled.push(format!("{}: clock not from a global buffer", ff.name));
                continue;
            };
            if ff.net("ena").is_some() && d.2.is_none() {
                unmodelled.push(format!("{}: clock enable not on a LAB control line", ff.name));
            }
            let v = lab_domains.entry((x, y)).or_default();
            if !v.contains(&d) {
                v.push(d);
            }
        }
        let mut domain_index: HashMap<((u32, u32), Domain), u8> = HashMap::new();
        for (&xy, v) in &lab_domains {
            let slot = clock_slots(v).with_context(|| format!("LAB {xy:?}"))?;
            let lab = labs.entry(xy).or_default();
            for (j, d) in slot.iter().enumerate() {
                if let Some(d) = d {
                    lab.clocks[j] = Some(GlobalClock { buf: d.0, inverted: d.1 });
                    lab.enables[j] = d.2;
                    domain_index.insert((xy, *d), j as u8);
                }
            }
        }

        for (&(x, y, le), &(comb, ff)) in &slots {
            let lab = labs.entry((x, y)).or_default();
            let mut c = LeConfig {
                x,
                y,
                le,
                lut: None,
                inputs: inputs.get(&(x, y, le)).copied().unwrap_or_default(),
                carry_in: false,
                reg_feedback: false,
                reg: None,
                comb_out: [false; 3],
            };
            if let Some(comb) = comb {
                c.lut = Some(Lut { mask: comb.lut_mask().with_context(|| format!("{}: no lut_mask", comb.name))? });
                c.carry_in = comb.params.get("sum_lutc_input").map(String::as_str) == Some("cin") && comb.net("cin").is_some();
                // Own register on datac without a routed input: the
                // register feedback path (counter LSB: `cnt[0]~75`).
                let own = ff.and_then(|f| f.port("regout"));
                c.reg_feedback = own.is_some() && comb.port("datac") == own && c.inputs[2].is_none();
            }
            if let Some(ff) = ff {
                let g = glob.get(&(x, y, le));
                let k = ctl.get(&(x, y, le));
                let clock = domain(x, y, le, ff).and_then(|d| domain_index.get(&((x, y), d))).copied().unwrap_or(0);
                if inverted(ff, "aclr") {
                    unmodelled.push(format!("{}: inverted aclr", ff.name));
                }
                let aclr = if ff.net("aclr").is_some() {
                    let a = match (k.and_then(|k| k.get("ACLR")), g.and_then(|g| g.get("ACLR"))) {
                        (Some(&p), _) => Aclr::Pia(p),
                        (None, Some(&b)) => Aclr::Global(b),
                        _ => bail!("{}: aclr route not found", ff.name),
                    };
                    set_once(&mut lab.aclr, a, "aclr", (x, y))?;
                    Some(0)
                } else {
                    None
                };
                let mut sync = false;
                if ff.net("sclr").is_some() {
                    let line = *k.and_then(|k| k.get("SCLR")).with_context(|| format!("{}: sclr route", ff.name))?;
                    set_once(&mut lab.sync.sclr, Ctl { line, inverted: inverted(ff, "sclr") }, "sync clear", (x, y))?;
                    sync = true;
                }
                match ff.port("sload") {
                    Some("vcc") => {
                        set_once(&mut lab.sync.sload, Sload::Const, "sync load", (x, y))?;
                        sync = true;
                    }
                    Some(n) if n != "gnd" => {
                        let line = *k.and_then(|k| k.get("SLOAD")).with_context(|| format!("{}: sload route", ff.name))?;
                        let c = Ctl { line, inverted: inverted(ff, "sload") };
                        set_once(&mut lab.sync.sload, Sload::Pia(c), "sync load", (x, y))?;
                        sync = true;
                    }
                    _ => {}
                }
                c.reg = Some(Reg { clock, aclr, sync });
            }
            let outs = outputs.get(&(x, y, le)).cloned().unwrap_or_default();
            let comb_names = comb.map(names).unwrap_or_default();
            for (o, net) in outs.iter().enumerate() {
                c.comb_out[o] = net.as_ref().is_some_and(|n| comb_names.contains(n));
            }
            out.les.push(c);
        }
        out.labs = labs;
        // Clock control: the rcf runs pin -> CLKBUF_IN (z, input) on one
        // net and CLKBUF_OUT (z) -> GLOBAL_CLK_H (index = G) on the next;
        // match them up by tile and z.
        let mut ins: HashMap<(i16, i16, i16), u8> = HashMap::new();
        let mut outs: HashMap<(i16, i16, i16), u8> = HashMap::new();
        for s in sigs {
            for (i, r) in s.resources.iter().enumerate() {
                match r.kind.as_str() {
                    "CLKBUF_IN" => {
                        ins.insert((r.x, r.y, r.z), r.index as u8);
                    }
                    "GLOBAL_CLK_H" => {
                        if let Some(p) = s.parents[i].map(|p| &s.resources[p]).filter(|p| p.kind == "CLKBUF_OUT") {
                            outs.insert((p.x, p.y, p.z), r.index as u8);
                        }
                    }
                    _ => {}
                }
            }
        }
        for (k, &g) in &outs {
            let input = *ins.get(k).with_context(|| format!("clock control at {k:?} has no CLKBUF_IN"))?;
            out.clkctrls.push(ClkCtrl { g, input });
        }
        Ok((out, unmodelled))
    }

    /// Every bit this logic fixes, as (flat CRAM address, value). Bits not
    /// listed keep the empty design's value.
    pub fn bits(&self, db: &LabDb) -> Result<Vec<(u32, bool)>> {
        let mut out = Vec::new();
        for c in &self.les {
            le_bits(db, c, &mut out)?;
        }
        for c in &self.clkctrls {
            // Unused controls read select 0b111 / enable 0 in the empty
            // design; in use: the input index, LSB first, and enable 1
            // (counter and fuzz designs, G2/G3/G6/G8/G10/G11/G15).
            let sel = &db.clkctrl_select[c.g as usize];
            for (k, &b) in sel.iter().enumerate() {
                out.push((b as u32, c.input >> k & 1 == 1));
            }
            out.push((db.clkctrl_enable[c.g as usize] as u32, true));
        }
        for (&(x, y), l) in &self.labs {
            lab_bits(db, x, y, l, &mut out).with_context(|| format!("LAB X{x} Y{y} {l:?}"))?;
        }
        Ok(out)
    }
}

/// A register's clock domain in a LAB: (row-clock buffer, inverted, enable
/// control line).
pub type Domain = (u8, bool, Option<Ctl>);

/// Which LAB clock (0/1) each of a LAB's clock domains takes. The bitstream
/// doesn't record the choice, so this reproduces the fitter's: CE 1 can
/// only come from control lines 0/1 and CE 0 from 2/3 (asmdb fast-mux
/// maps), and an LE's clock select picks clock and enable together, so an
/// enabled domain's line fixes its LAB clock; the rest fill in inverted
/// clocks first, then lowest buffer (fit to the two-clock fuzz LABs: 6
/// non-inverted pairs, 1 inverted/non-inverted pair).
pub fn clock_slots(domains: &[Domain]) -> Result<[Option<Domain>; 2]> {
    let mut v = domains.to_vec();
    v.sort_by_key(|d| (!d.1, d.0, d.2));
    v.dedup();
    ensure!(v.len() <= 2, "{} clock domains", v.len());
    let mut slot: [Option<Domain>; 2] = [None, None];
    for d in v.iter().filter(|d| d.2.is_some()) {
        let j = if d.2.unwrap().line < 2 { 1 } else { 0 };
        ensure!(slot[j].is_none(), "two enables want clock {j}");
        slot[j] = Some(*d);
    }
    for d in v.iter().filter(|d| d.2.is_none()) {
        let j = slot.iter().position(Option::is_none).unwrap();
        slot[j] = Some(*d);
    }
    Ok(slot)
}

fn inverted(a: &Atom, port: &str) -> bool {
    a.port(port).is_some_and(|n| n.starts_with('!'))
}

fn set_once<T: PartialEq + std::fmt::Debug>(slot: &mut Option<T>, v: T, what: &str, lab: (u32, u32)) -> Result<()> {
    match slot {
        Some(old) if *old != v => bail!("LAB {lab:?}: two {what}s ({old:?}, {v:?}), only one modelled"),
        _ => *slot = Some(v),
    }
    Ok(())
}

fn le_bits(db: &LabDb, c: &LeConfig, out: &mut Vec<(u32, bool)>) -> Result<()> {
    let (x, y, l) = (c.x, c.y, c.le as usize);
    let mut put = |off: i32, v: bool| -> Result<()> {
        out.push((db.flat(x, y, off)?, v));
        Ok(())
    };
    if let Some(lut) = c.lut {
        // Setting i is truth-table entry 15 - i; stored inverted.
        let f = db.feature(arch::LE_LUT_FUNCTION, -1)?;
        for (i, &off) in f.paths[l][0].iter().enumerate() {
            put(off, lut.mask >> (15 - i) & 1 == 0)?;
        }
    }
    for (p, src) in c.inputs.iter().enumerate() {
        if let Some(src) = src {
            for off in db.feature(arch::LE_LUT_INPUT, p as i32)?.source_bits(l, src.index())? {
                put(off, true)?;
            }
        }
    }
    put(db.feature(arch::LE_USE_CARRY, -1)?.bit(l), c.carry_in)?;
    put(db.feature(arch::LE_USE_REGISTER_FEEDBACK, -1)?.bit(l), c.reg_feedback)?;
    for (o, &comb) in c.comb_out.iter().enumerate() {
        put(db.feature(arch::LE_REG_OUT_YGR, o as i32)?.bit(l), comb)?;
    }
    if let Some(r) = c.reg {
        // Unused LEs read LE_CLOCK_SELECT 1 and LE_CLEAR_SELECT 0 in the
        // empty design; a register without a clear selects line 1 (fuzz).
        put(db.feature(arch::LE_CLOCK_SELECT, -1)?.bit(l), r.clock == 1)?;
        put(db.feature(arch::LE_CLEAR_SELECT, -1)?.bit(l), r.aclr.unwrap_or(1) == 1)?;
        // A decision mux whose only real bit is the per-LE "use LAB sync
        // signals" enable; LE_USE_SYNC_LOAD names the same bit.
        let f = db.feature(arch::LE_USE_SYNC_CLEAR, -1)?;
        let bit = *f.paths[l][0].iter().find(|&&b| b >= 0).context("sync decision mux has no bit")?;
        put(bit, r.sync)?;
    }
    Ok(())
}

/// Fuzz-read values of the LAB-wide groups. Where a group is a source mux
/// the setting is the asmdb's map of the control line; the polarities
/// (enable bits that go *low* when used, the 3-bit sync code) are observed.
fn lab_bits(db: &LabDb, x: u32, y: u32, l: &LabConfig, out: &mut Vec<(u32, bool)>) -> Result<()> {
    let mut put = |off: i32, v: bool| -> Result<()> {
        out.push((db.flat(x, y, off)?, v));
        Ok(())
    };
    for (k, src) in l.pia.iter().enumerate() {
        if let Some(src) = src {
            for off in db.feature(arch::LAB_PIA_TO_INPUT, k as i32)?.source_bits(0, src.index())? {
                put(off, true)?;
            }
        }
    }
    for (j, clk) in l.clocks.iter().enumerate() {
        let Some(clk) = clk else { continue };
        // Sources 4..9 of LAB_CLOCK_FAST_MUX are the six BLK_CLK_BUFs.
        let f = db.feature(arch::LAB_CLOCK_FAST_MUX, j as i32)?;
        put(f.source_bits(0, 4 + clk.buf as usize)?[0], true)?;
        // LAB_INVERT 0 / 2 (rows 32 and 37, column 0): clock inversion.
        put(db.feature(arch::LAB_INVERT, 2 * j as i32)?.bit(0), clk.inverted)?;
    }
    for (j, en) in l.enables.iter().enumerate() {
        // LAB_ENABLE_CE j and LAB_INVERT 1 / 3 read 1 when clock j has no
        // enable and 0 when it has one (fuzz); the CE fast mux setting is
        // the control line's entry in the asmdb map.
        put(db.feature(arch::LAB_ENABLE_CE, j as i32)?.bit(0), en.is_none())?;
        put(db.feature(arch::LAB_INVERT, 2 * j as i32 + 1)?.bit(0), !en.is_some_and(|c| !c.inverted))?;
        if let Some(c) = en {
            let f = db.feature(arch::LAB_CE_FAST_MUX, j as i32)?;
            put(f.bit(0), encoded_setting(f, c.line as usize)? == 1)?;
        }
    }
    match l.aclr {
        None => {}
        Some(Aclr::Pia(p)) => {
            // 2-bit encoded first level: setting s sets bit s - 1.
            let f = db.feature(arch::LAB_CLEAR_FAST_MUX, 0)?;
            let s = encoded_setting(f, p as usize)?;
            for (i, &off) in f.paths[0][0].iter().enumerate() {
                put(off, s == i + 1)?;
            }
        }
        Some(Aclr::Global(b)) => {
            // Both clear lines take the global and the pair inversion is
            // set (fuzz: X2Y26, X54Y16); sources 4..9 are BLK_CLK_BUFs.
            for j in 0..2 {
                let f = db.feature(arch::LAB_CLEAR_FAST_MUX, j)?;
                let s = encoded_setting(f, 4 + b as usize)?;
                for (i, &off) in f.paths[0][0].iter().enumerate() {
                    put(off, s == i + 1)?;
                }
                let n = f.next.as_ref().context("clear mux second level")?[4 + b as usize];
                ensure!(n >= 0, "global {b} unreachable");
                put(f.paths[0][1][n as usize], true)?;
            }
            for &off in &db.feature(arch::LAB_CLR_INVERT_PAIR, 6)?.paths[0][0] {
                put(off, true)?;
            }
        }
    }
    // LAB_ENABLE_SYNC_SIGNALS: 3 bits, MSB first, empty/none = 0b101.
    // Codes are the asmdb setting map [5, 7, 4, 6, 1, 0]; which setting is
    // which combination was read off fuzz LABs (sclr+sload: see README).
    let (sclr, sload) = (l.sync.sclr, l.sync.sload);
    let code = match (sclr, sload) {
        (None, None) => 0b101,
        (None, Some(Sload::Pia(_))) => 0b111,
        (None, Some(Sload::Const)) => 0b100,
        (Some(_), None) => 0b001,
        (Some(_), Some(Sload::Pia(_))) => 0b011,
        (Some(_), Some(Sload::Const)) => 0b000,
    };
    let sload = match sload {
        Some(Sload::Pia(p)) => Some(p),
        _ => None,
    };
    let f = db.feature(arch::LAB_ENABLE_SYNC_SIGNALS, -1)?;
    for (i, &off) in f.paths[0][0].iter().enumerate() {
        put(off, code >> (2 - i) & 1 == 1)?;
    }
    // LAB_INVERT 5 is set by a sync clear and LAB_INVERT 4 cleared by a
    // sync load, unless the register port inverts the signal (fuzz, and the
    // counter's `!KEY[0]` clear): the line itself is active low.
    put(db.feature(arch::LAB_INVERT, 5)?.bit(0), sclr.is_some_and(|c| !c.inverted))?;
    put(db.feature(arch::LAB_INVERT, 4)?.bit(0), !sload.is_some_and(|c| !c.inverted))?;
    let _ = &l.sync;
    if let Some(c) = sclr {
        let f = db.feature(arch::LAB_SYNC_CLR_FAST_MUX, 0)?;
        put(f.bit(0), encoded_setting(f, c.line as usize)? == 1)?;
    }
    if let Some(c) = sload {
        let f = db.feature(arch::LAB_SYNC_LOAD_FAST_MUX_MAG, 0)?;
        put(f.bit(0), encoded_setting(f, c.line as usize)? == 1)?;
    }
    Ok(())
}

/// Setting of an encoded fast mux for source `s` (its asmdb aux map).
fn encoded_setting(f: &crate::lab::Feature, s: usize) -> Result<usize> {
    let a = *f.aux.as_ref().context("no source map")?.get(s).context("source out of range")?;
    ensure!(a >= 0, "source {s} not selectable (setting {a})");
    Ok(a as usize)
}
