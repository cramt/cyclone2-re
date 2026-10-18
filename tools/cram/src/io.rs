//! I/O pins, from first principles: package pin -> pad -> IOE (ddb `pkg`
//! and `pad`), IOE -> the bits of every IOE feature (ddb `asmdb` groups of
//! block types 5 = row IOE, 6 = column IOE, 60/61 = dedicated clock input
//! p/n), and per pin state -> feature values (learned from the I/O fuzz
//! designs, `tools/gen_io.py`; README "I/O pins").
//!
//! An IOE's features live in two places. Core CRAM bits are plain flat
//! addresses. Drive strength, pull-up, bus hold, I/O standard and pin mode
//! are "CFF" bits (asmdb bit value `0xa0000000 | k`) that sit in the 6
//! replicated frames at the front of the stream (`rbf::RepBit`).
//!
//! Assembly writes every bit of every IOE's footprint, so the result
//! doesn't depend on which pins the base image happened to use. The one
//! dual-purpose configuration pins (ASDO, nCSO: inputs with pull-up; nCEO:
//! an output), which Quartus configures from device options when the
//! design doesn't claim them, at their bank's I/O standard.

use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use ddb::asmdb::{ArchBody, Asmdb, MuxId};

use crate::rbf::{FrameBit, Rbf, RepBit};
use crate::{addr, rcf, vo};

/// asmdb block types of the IOE-like blocks.
pub const BLOCK_ROW: i32 = 5;
pub const BLOCK_COL: i32 = 6;
pub const BLOCK_CLK_P: i32 = 60;
pub const BLOCK_CLK_N: i32 = 61;

/// `ASMDB_ARCH_GROUP_TYPE` values of the features the pin model sets
/// (names from the libdb_asmdb.so translator table).
mod arch {
    pub const IO_PIN_MODE: i32 = 84;
    pub const IO_DATA_INPUT: i32 = 85;
    pub const IO_SELECT_IOST_DIRT: i32 = 96;
    pub const IO_PIA_TO_OE: i32 = 105;
    pub const IO_REG_FEEDS_PIN_OUT_YGR: i32 = 109;
    pub const IO_INREG_CLK_FAST_MUX: i32 = 89;
    pub const IO_OUTREG_CLK_FAST_MUX: i32 = 93;
    pub const IO_DATA_INVERT: i32 = 118;
    pub const IO_USE_CLR_INREG: i32 = 164;
    pub const IO_USE_CLR_OEREG: i32 = 165;
    pub const IO_USE_CLR_OUTREG: i32 = 166;
    pub const IO_OE_INVERT: i32 = 122;
    pub const IO_SET_CE_TO_OEREG_TIE_OFF: i32 = 130;
    pub const IO_SET_CE_TO_OUTREG_TIE_OFF: i32 = 131;
    pub const IO_SET_PAD_TO_CORE_DUAL_FUNC_DELAY_MAG: i32 = 139;
    pub const IO_SET_DRIVE_STRENGTH_ARMSTRONG: i32 = 161;
    pub const IO_USE_BUS_HOLD: i32 = 163;
    pub const IO_USE_PULL_UP: i32 = 176;
    pub const IO_USE_OUTPUT_DUTY_CYCLE: i32 = 177;
}

/// A configuration bit an IOE owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IoBit {
    Core(FrameBit),
    Rep(RepBit),
}

impl IoBit {
    pub fn get(self, r: &Rbf) -> bool {
        match self {
            IoBit::Core(b) => r.get(b),
            IoBit::Rep(b) => r.rep_get(b),
        }
    }
    pub fn set(self, r: &mut Rbf, v: bool) {
        match self {
            IoBit::Core(b) => r.set(b, v),
            IoBit::Rep(b) => r.rep_set(b, v),
        }
    }
}

/// CFF bit k -> replicated-frame bit. CFF bits come in blocks of 24 (one
/// per IOE, PLL, ...); block j sits in bit-in-byte lane `lane` of the
/// replicated frames at linear byte slot `u = k - off`, with 311 usable
/// bytes per frame starting at byte 1. Fit to the unused-pin pull-up design
/// (455 IOE pull-up bits) and the 16 dedicated clock inputs; every IOE bit
/// of every I/O fuzz design lands where this puts it. `(lane, first
/// block, off)`.
const CFF_LANES: [(u16, u32, i64); 8] =
    [(7, 0, -216), (6, 64, 1176), (5, 126, 2808), (4, 190, 4440), (3, 266, 6024), (2, 324, 7368), (1, 388, 9120), (0, 452, 10704)];

pub fn cff_bit(k: u32) -> Result<RepBit> {
    let j = k / 24;
    let &(lane, _, off) = CFF_LANES.iter().rev().find(|(_, j0, _)| j >= *j0).unwrap();
    let u = k as i64 - off;
    ensure!((0..6 * 311).contains(&u), "CFF bit {k} falls outside the replicated frames");
    Ok(RepBit { frame: (u / 311) as u8, bit: ((u % 311 + 1) * 8) as u16 + lane })
}

/// asmdb bit value -> bit, `None` for the "no such bit" sentinel.
fn bit_of(v: i32) -> Result<Option<IoBit>> {
    if v >= 0 {
        return Ok(Some(IoBit::Core(addr::frame_bit(v as u32))));
    }
    // -3: the slot exists in the group's layout but not on this IOE.
    if v == -3 {
        return Ok(None);
    }
    ensure!(v as u32 >> 24 == 0xa0, "asmdb IOE bit {v:#x} is neither a flat address nor a CFF bit");
    let k = v as u32 & 0xff_ffff;
    Ok(Some(IoBit::Rep(cff_bit(k)?)))
}

/// Which asmdb instance a package pin's IOE is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ioe {
    pub block: i32,
    /// Path index in that block type's groups.
    pub path: usize,
}

/// A two-level source mux (output data, output enable): per source, the
/// first- and second-level bit index (asmdb `aux` / `next_mux_selection`),
/// and per path the bits of each level.
struct SourceMux {
    sel: Vec<(i32, i32)>,
    levels: Vec<[Vec<i32>; 2]>,
}

impl SourceMux {
    fn bits(&self, path: usize, source: u32) -> Result<[i32; 2]> {
        let &(a, n) = self.sel.get(source as usize).context("source out of range")?;
        ensure!(a >= 0 && n >= 0, "source {source} can't reach this mux");
        let l = &self.levels[path];
        Ok([l[0][a as usize], l[1][n as usize]])
    }
}

pub struct IoDb {
    pub pins: HashMap<String, Pin>,
    /// (block, arch type, group index) -> per path, the feature's bits in
    /// level order (`None`: a sentinel slot the hardware doesn't have).
    features: BTreeMap<(i32, i32, i32), Vec<Vec<Option<IoBit>>>>,
    muxes: HashMap<(i32, i32), SourceMux>,
}

#[derive(Debug, Clone)]
pub struct Pin {
    pub name: String,
    pub ioe: Ioe,
    pub xyz: (u16, u16, u8),
    pub bank: i32,
    pub dual_purpose: Option<DualPurpose>,
}

/// Configuration pins that double as user I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DualPurpose {
    /// ASDO, nCSO: active serial flash interface, inputs with pull-up.
    SerialFlash,
    /// nCEO: chip-enable out, an output.
    Nceo,
}

fn mux_bits(db: &Asmdb, m: &Option<MuxId>) -> Vec<i32> {
    m.map(|m| db.mux(m).bits.clone()).unwrap_or_default()
}

impl IoDb {
    pub fn new(devinfo: &Path, db: &Asmdb) -> Result<Self> {
        let (pkg, pad) = ddb::pins::load(devinfo, "magellan4", "fbga672")?;
        // Path order is pad-id order within each pad type; dedicated clock
        // inputs pair up in pad order, the p pad in block 60 and n in 61.
        // Observed: every output-data mux of the I/O fuzz designs is on the
        // path this predicts.
        let mut pads: Vec<_> = pad.pads.iter().filter(|p| p.loc.is_some()).collect();
        pads.sort_by_key(|p| p.id);
        let mut ioe_of = HashMap::new();
        let (mut row, mut col, mut clk) = (0, 0, 0);
        for p in &pads {
            let ioe = match p.io_type.as_deref() {
                Some("Row I/O") => (BLOCK_ROW, &mut row),
                Some("Column I/O") => (BLOCK_COL, &mut col),
                Some("Dedicated Clock") => {
                    let n = p.alt_name.as_deref().is_some_and(|a| a.ends_with('A'));
                    let path = clk / 2;
                    clk += 1;
                    ioe_of.insert(p.id, Ioe { block: if n { BLOCK_CLK_N } else { BLOCK_CLK_P }, path });
                    continue;
                }
                _ => continue,
            };
            ioe_of.insert(p.id, Ioe { block: ioe.0, path: *ioe.1 });
            *ioe.1 += 1;
        }
        let mut pins = HashMap::new();
        for m in ddb::pins::join(&pkg, &pad)? {
            let [p] = m.pads.as_slice() else { continue };
            let Some(&ioe) = ioe_of.get(&p.id) else { continue };
            let f = p.function.as_deref().unwrap_or("");
            let dual_purpose = if f.contains("nCEO") {
                Some(DualPurpose::Nceo)
            } else if f.contains("ASDO") || f.contains("nCSO") {
                Some(DualPurpose::SerialFlash)
            } else {
                None
            };
            pins.insert(m.pin.name.clone(), Pin { name: m.pin.name.clone(), ioe, xyz: p.loc.unwrap(), bank: p.bank, dual_purpose });
        }

        let mut features = BTreeMap::new();
        let mut muxes = HashMap::new();
        for g in db.arch_groups.iter().filter(|g| [BLOCK_ROW, BLOCK_COL, BLOCK_CLK_P, BLOCK_CLK_N].contains(&g.block_type)) {
            let per_path: Vec<Vec<Option<IoBit>>> = match &g.body {
                // A dumb group lists one bit per path, in path order
                // (checked: its pull-up/bus-hold blocks agree with the
                // muxed groups' drive-strength blocks on all 459 IOEs).
                ArchBody::Dumb { bits, .. } => bits.iter().map(|&b| Ok(vec![bit_of(b)?])).collect::<Result<_>>()?,
                _ => g
                    .paths()
                    .iter()
                    .map(|p| p.iter().flat_map(|m| mux_bits(db, m)).map(bit_of).collect::<Result<_>>())
                    .collect::<Result<_>>()?,
            };
            ensure!(
                features.insert((g.block_type, g.arch_type, g.group_index), per_path).is_none(),
                "duplicate IOE group {} {}",
                g.block_type,
                g.arch_type
            );
            if [arch::IO_DATA_INPUT, arch::IO_PIA_TO_OE].contains(&g.arch_type) {
                let d = g.data().first().context("source mux without data")?;
                let (aux, next) = (mux_bits(db, &d.aux), mux_bits(db, &d.next_selection));
                let levels = g
                    .paths()
                    .iter()
                    .map(|p| match p.as_slice() {
                        [a, b] => Ok([mux_bits(db, a), mux_bits(db, b)]),
                        _ => bail!("source mux path is not two levels"),
                    })
                    .collect::<Result<_>>()?;
                muxes.insert((g.block_type, g.arch_type), SourceMux { sel: aux.into_iter().zip(next).collect(), levels });
            }
        }
        Ok(IoDb { pins, features, muxes })
    }

    /// Every bit of one IOE, by feature.
    fn footprint(&self, ioe: Ioe) -> impl Iterator<Item = ((i32, i32), &[Option<IoBit>])> {
        self.features
            .range((ioe.block, i32::MIN, i32::MIN)..=(ioe.block, i32::MAX, i32::MAX))
            .filter_map(move |(&(_, a, i), v)| v.get(ioe.path).map(|b| ((a, i), b.as_slice())))
    }

    /// Bits of feature (arch, group index) for one IOE.
    fn feature(&self, ioe: Ioe, arch: i32, index: i32) -> Result<&[Option<IoBit>]> {
        self.features
            .get(&(ioe.block, arch, index))
            .and_then(|v| v.get(ioe.path))
            .map(Vec::as_slice)
            .with_context(|| format!("IOE {ioe:?} has no feature {arch}/{index}"))
    }
}

/// I/O standards the model knows (single-ended, no VREF).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standard {
    Lvttl33,
    Lvcmos33,
    V25,
    V18,
    V15,
}

impl Standard {
    /// Row IOE default current (what Quartus gives nCEO).
    fn row_default_ma(self) -> u32 {
        match self {
            Standard::Lvttl33 => 24,
            Standard::Lvcmos33 => 24,
            Standard::V25 => 8,
            Standard::V18 => 12,
            Standard::V15 => 6,
        }
    }
    fn low_voltage_input(self) -> bool {
        matches!(self, Standard::V18 | Standard::V15)
    }
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "3.3-V LVTTL" => Standard::Lvttl33,
            "3.3-V LVCMOS" => Standard::Lvcmos33,
            "2.5 V" => Standard::V25,
            "1.8 V" => Standard::V18,
            "1.5 V" => Standard::V15,
            _ => bail!("I/O standard {s:?} isn't modelled"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    In,
    Out,
    Bidir,
}

/// How the design uses one pin.
#[derive(Debug, Clone)]
pub struct PinUse {
    pub pin: String,
    pub port: String,
    pub role: Role,
    pub standard: Standard,
    /// mA, outputs and bidirs only.
    pub drive: Option<u32>,
    pub pull_up: bool,
    pub bus_hold: bool,
    /// LAB-local line into the output data / output enable mux. `None`
    /// with a constant: `data_const`.
    pub data_line: Option<u32>,
    pub data_const: Option<bool>,
    pub oe_line: Option<u32>,
    /// Which of the two pad-to-core paths (`IO_DATAIN` index) the input
    /// drives the core through.
    pub datain_used: [bool; 2],
    /// I/O-cell registers, each with the I/O row clock buffer
    /// (`BLK_CLK_BUF` index) that clocks it. The OE register shares the
    /// output clock.
    pub in_reg: Option<u32>,
    pub out_reg: Option<u32>,
    pub oe_reg: bool,
}

/// `set_instance_assignment -name <NAME> ON -to <port>` in a qsf.
fn qsf_on(qsf: &str, name: &str) -> Vec<String> {
    qsf.lines()
        .filter_map(|l| {
            let r = l.trim().strip_prefix("set_instance_assignment -name ")?.strip_prefix(name)?.trim_start();
            let r = r.strip_prefix("ON -to ")?;
            Some(r.trim().trim_matches('"').to_string())
        })
        .collect()
}

/// Pin usage from the post-fit netlist (`cycloneii_io` atoms: location,
/// mode, current strength), the back-annotated qsf (pull-up, bus hold)
/// and the rcf (which local lines feed the pin, which input path it uses).
pub fn pin_uses(atoms: &[vo::Atom], sigs: &[rcf::Signal], qsf: &str) -> Result<Vec<PinUse>> {
    let pull = qsf_on(qsf, "WEAK_PULL_UP_RESISTOR");
    let hold = qsf_on(qsf, "ENABLE_BUS_HOLD_CIRCUITRY");
    // rcf sinks into pins: (port, DATAIN|OE) -> local line feeding it.
    let mut lines: HashMap<(String, String), u32> = HashMap::new();
    let mut datain: HashMap<String, [bool; 2]> = HashMap::new();
    // (port, INCLK|OUTCLK) -> I/O clock buffer index.
    let mut clocks: HashMap<(String, String), u32> = HashMap::new();
    for s in sigs {
        for d in &s.dests {
            let Some(from) = d.from else { continue };
            if ["INCLK", "OUTCLK"].contains(&d.port.as_str()) {
                let r = &s.resources[from];
                ensure!(r.kind == "BLK_CLK_BUF", "{}: I/O register clock from {}", s.name, r.kind);
                clocks.insert((d.cell.clone(), d.port.clone()), r.index as u32);
                continue;
            }
            if !["IO_DATAOUT", "IO_OE_PIN"].contains(&s.resources[from].kind.as_str()) {
                continue;
            }
            let l = s.parents[from].map(|p| &s.resources[p]).filter(|r| r.kind == "LOCAL_INTERCONNECT");
            let l = l.with_context(|| format!("{}: pin sink not fed by a local line", s.name))?;
            lines.insert((d.cell.clone(), d.port.clone()), l.index as u32);
        }
        for r in s.resources.iter().filter(|r| r.kind == "IO_DATAIN") {
            ensure!((0..2).contains(&r.index), "IO_DATAIN index {}", r.index);
            datain.entry(s.name.clone()).or_default()[r.index as usize] = true;
        }
    }
    let mut out = Vec::new();
    for a in atoms.iter().filter(|a| a.kind == "io") {
        let pin = a.loc.strip_prefix("PIN_").with_context(|| format!("{}: location {}", a.name, a.loc))?.to_string();
        let port = a.port("padio").with_context(|| format!("{}: no padio", a.name))?.to_string();
        let role = match a.params.get("operation_mode").map(String::as_str) {
            Some("input") => Role::In,
            Some("output") => Role::Out,
            Some("bidir") => Role::Bidir,
            m => bail!("{}: operation_mode {m:?}", a.name),
        };
        let standard = Standard::parse(a.note("I/O Standard").with_context(|| format!("{}: no I/O standard", a.name))?)?;
        let drive = match a.note("Current Strength") {
            None | Some("Default") => None,
            Some(s) => Some(s.strip_suffix("mA").and_then(|n| n.parse().ok()).with_context(|| format!("{}: current {s}", a.name))?),
        };
        let data_line = lines.get(&(port.clone(), "DATAIN".into())).copied();
        let reg = |mode: &str| a.params.get(mode).is_some_and(|m| m == "register");
        for p in ["areset", "sreset"] {
            ensure!(a.net(p).is_none(), "{}: I/O register {p} isn't modelled", a.name);
        }
        for p in ["inclkena", "outclkena"] {
            ensure!(a.port(p) == Some("vcc"), "{}: I/O register {p} isn't modelled", a.name);
        }
        let clock = |c: &str| clocks.get(&(port.clone(), c.to_string())).copied().with_context(|| format!("{}: {c} not in the rcf", a.name));
        let in_reg = if reg("input_register_mode") { Some(clock("INCLK")?) } else { None };
        let out_reg = if reg("output_register_mode") { Some(clock("OUTCLK")?) } else { None };
        let oe_reg = reg("oe_register_mode");
        ensure!(!oe_reg || out_reg.is_some(), "{}: OE register without output register isn't modelled", a.name);
        let data_const = match a.port("datain") {
            Some("gnd") => Some(false),
            Some("vcc") => Some(true),
            _ => None,
        };
        out.push(PinUse {
            pin,
            role,
            standard,
            drive,
            pull_up: pull.contains(&port),
            bus_hold: hold.contains(&port),
            data_line,
            data_const,
            oe_line: lines.get(&(port.clone(), "OE".into())).copied(),
            // The rcf names the input's core net: the pin itself for a
            // combinational input, the register output for a registered one.
            datain_used: [Some(port.as_str()), a.net("regout"), a.net("combout")]
                .into_iter()
                .flatten()
                .filter_map(|n| datain.get(n))
                .fold([false; 2], |acc, d| [acc[0] | d[0], acc[1] | d[1]]),
            port,
            in_reg,
            out_reg,
            oe_reg,
        });
    }
    Ok(out)
}

/// One feature value: bits in the feature's level order.
type Setting = (i32, i32, &'static [u8]);

/// What unused user I/O pins do (`RESERVE_ALL_UNUSED_PINS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnusedPins {
    InputTriStated,
    InputBusHold,
    InputPullUp,
    /// Also what "AS OUTPUT DRIVING AN UNSPECIFIED SIGNAL" gives: the
    /// two came out bit-identical.
    OutputGround,
}

impl UnusedPins {
    /// From a qsf; absent means our oracle's setting, input tri-stated.
    pub fn from_qsf(qsf: &str) -> Result<Self> {
        let mut mode = UnusedPins::InputTriStated;
        for l in qsf.lines() {
            let Some(v) = l.trim().strip_prefix("set_global_assignment -name RESERVE_ALL_UNUSED_PINS ") else { continue };
            mode = match v.trim().trim_matches('"') {
                "AS INPUT TRI-STATED" => UnusedPins::InputTriStated,
                "AS INPUT TRI-STATED WITH BUS-HOLD" => UnusedPins::InputBusHold,
                "AS INPUT TRI-STATED WITH WEAK PULL-UP" => UnusedPins::InputPullUp,
                "AS OUTPUT DRIVING GROUND" | "AS OUTPUT DRIVING AN UNSPECIFIED SIGNAL" => UnusedPins::OutputGround,
                m => bail!("RESERVE_ALL_UNUSED_PINS {m:?} isn't modelled"),
            };
        }
        Ok(mode)
    }
}

/// Feature values of an unused IOE, observed on every unused IOE of the
/// fuzz designs (one design per mode) apart from the dual-purpose pins.
/// Features not listed are 0.
fn unused_settings(block: i32, mode: UnusedPins) -> Vec<Setting> {
    use arch::*;
    let mut v: Vec<Setting> = vec![(IO_SELECT_IOST_DIRT, -1, &[1, 1, 0])];
    if matches!(block, BLOCK_ROW | BLOCK_COL) {
        v.extend([(IO_PIN_MODE, -1, &[1u8, 0][..]), (IO_REG_FEEDS_PIN_OUT_YGR, -1, &[1, 0][..])]);
        match mode {
            UnusedPins::OutputGround => v.push((IO_SET_DRIVE_STRENGTH_ARMSTRONG, -1, drive_code(block, Standard::Lvttl33, 24).unwrap())),
            _ => v.push((IO_OE_INVERT, -1, &[1])),
        }
        match mode {
            UnusedPins::InputBusHold => v.push((IO_USE_BUS_HOLD, -1, &[1])),
            UnusedPins::InputPullUp => v.push((IO_USE_PULL_UP, -1, &[1])),
            _ => {}
        }
    }
    v
}

/// An unclaimed dual-purpose pin at its bank's standard. Observed on the
/// fuzz designs at every standard (nCEO is a row IOE).
fn dual_purpose_settings(block: i32, d: DualPurpose, std: Standard) -> Result<Vec<Setting>> {
    use arch::*;
    let mut v: Vec<Setting> = vec![
        (IO_PIN_MODE, -1, &[1, 0]),
        (IO_SET_CE_TO_OEREG_TIE_OFF, -1, &[1]),
        (IO_SET_CE_TO_OUTREG_TIE_OFF, -1, &[1]),
    ];
    match d {
        DualPurpose::SerialFlash => {
            v.push((IO_OE_INVERT, -1, &[1]));
            v.push((IO_USE_PULL_UP, -1, &[1]));
            v.push((IO_SELECT_IOST_DIRT, -1, if std.low_voltage_input() { &[0, 1, 0] } else { &[0, 0, 0] }));
        }
        DualPurpose::Nceo => {
            v.push((IO_DATA_INVERT, -1, &[1]));
            v.push((IO_USE_OUTPUT_DUTY_CYCLE, -1, &[1, 1]));
            v.push((IO_SELECT_IOST_DIRT, -1, &[1, 1, 0]));
            // nCEO follows the bank's VCCIO, not its standard: in a 3.3-V
            // LVCMOS bank it still gets the LVTTL 24 mA code.
            let std = if std == Standard::Lvcmos33 { Standard::Lvttl33 } else { std };
            let code = drive_code(block, std, std.row_default_ma()).with_context(|| format!("nCEO drive at {std:?}"))?;
            v.push((IO_SET_DRIVE_STRENGTH_ARMSTRONG, -1, code));
        }
    }
    Ok(v)
}

/// Drive strength codes (`IO_SET_DRIVE_STRENGTH_ARMSTRONG`), observed on
/// the fuzz designs: every current of 3.3-V LVTTL, the default current of
/// the other standards. The code depends on the standard, not only the
/// current. Row IOEs have 8 bits, column IOEs 10.
fn drive_code(block: i32, std: Standard, ma: u32) -> Option<&'static [u8]> {
    use Standard::*;
    Some(match (block, std, ma) {
        (BLOCK_ROW, Lvttl33, 4) => &[1, 0, 0, 0, 1, 0, 0, 0],
        (BLOCK_ROW, Lvttl33, 8) => &[0, 1, 0, 0, 0, 1, 0, 0],
        (BLOCK_ROW, Lvttl33, 12) => &[1, 1, 0, 0, 1, 1, 0, 0],
        (BLOCK_ROW, Lvttl33, 16) => &[1, 0, 1, 0, 0, 0, 1, 0],
        (BLOCK_ROW, Lvttl33, 20) => &[0, 1, 1, 0, 1, 0, 1, 0],
        (BLOCK_ROW, Lvttl33, 24) => &[1, 1, 1, 0, 0, 1, 1, 0],
        (BLOCK_ROW, Lvcmos33, 12) => &[1, 1, 1, 0, 1, 1, 1, 0],
        (BLOCK_ROW, V25, 8) => &[1, 1, 0, 0, 0, 1, 1, 0],
        (BLOCK_ROW, V18, 12) => &[1, 1, 1, 0, 1, 1, 1, 0],
        (BLOCK_ROW, V15, 6) => &[0, 1, 1, 0, 0, 1, 1, 0],
        (BLOCK_COL, Lvttl33, 4) => &[1, 0, 0, 0, 0, 1, 0, 0, 0, 0],
        (BLOCK_COL, Lvttl33, 8) => &[0, 1, 0, 0, 0, 0, 1, 0, 0, 0],
        (BLOCK_COL, Lvttl33, 12) => &[1, 1, 0, 0, 0, 1, 1, 0, 0, 0],
        (BLOCK_COL, Lvttl33, 16) => &[1, 0, 1, 0, 0, 0, 0, 1, 0, 0],
        (BLOCK_COL, Lvttl33, 20) => &[0, 1, 1, 0, 0, 1, 0, 1, 0, 0],
        (BLOCK_COL, Lvttl33, 24) => &[1, 1, 1, 0, 0, 0, 1, 1, 0, 0],
        (BLOCK_COL, Lvcmos33, 24) => &[1, 1, 1, 1, 1, 1, 1, 0, 1, 1],
        (BLOCK_COL, V25, 16) => &[0, 1, 1, 0, 0, 0, 0, 1, 1, 0],
        (BLOCK_COL, V18, 12) => &[1, 1, 1, 0, 0, 1, 1, 1, 0, 0],
        (BLOCK_COL, V15, 8) => &[0, 0, 0, 1, 0, 0, 0, 0, 1, 0],
        _ => return None,
    })
}

/// Feature values of a used IOE, all observed on the fuzz designs.
fn used_settings(block: i32, u: &PinUse) -> Result<Vec<Setting>> {
    use arch::*;
    let mut v: Vec<Setting> = Vec::new();
    if matches!(block, BLOCK_ROW | BLOCK_COL) {
        // Pin mode: an OE register selects mode code 0 instead of 1
        // (asmdb map 1,1,1,3,3,0,2; which setting is which is unknown).
        v.push((IO_PIN_MODE, -1, if u.oe_reg { &[0, 0] } else { &[1, 0] }));
        v.extend([
            (IO_SET_CE_TO_OEREG_TIE_OFF, -1, &[1][..]),
            (IO_SET_CE_TO_OUTREG_TIE_OFF, -1, &[1][..]),
        ]);
        if u.role == Role::In {
            v.push((IO_OE_INVERT, -1, &[1]));
        }
        // Set on every used pin except outputs below 3.3 V (observed on
        // 2.5, 1.8 and 1.5 V; the name suggests duty-cycle correction).
        let low_v = matches!(u.standard, Standard::V25 | Standard::V18 | Standard::V15);
        if u.role == Role::In || !low_v {
            v.push((IO_USE_OUTPUT_DUTY_CYCLE, -1, &[1, 1]));
        }
        if u.role != Role::In {
            // The netlist names the current of every output; "Default" is
            // only seen on 3.3-V LVTTL, where it is 24 mA. (Other
            // standards' defaults differ between row and column IOEs.)
            let ma = match (u.drive, u.standard) {
                (Some(ma), _) => ma,
                (None, Standard::Lvttl33) => 24,
                (None, s) => bail!("{}: {s:?} output with no current strength", u.pin),
            };
            let code = drive_code(block, u.standard, ma)
                .with_context(|| format!("{}: no drive code for {:?} at {ma} mA", u.pin, u.standard))?;
            v.push((IO_SET_DRIVE_STRENGTH_ARMSTRONG, -1, code));
            if u.data_line.is_none() {
                match u.data_const {
                    // Observed on the empty design's LED tied to 0.
                    Some(false) => v.push((IO_DATA_INVERT, -1, &[1])),
                    // An unselected data mux drives 0, so a 1 needs no
                    // inversion; observed on bidirs whose data folded to vcc.
                    Some(true) => {}
                    None => bail!("{}: output data neither routed nor tied to ground", u.pin),
                }
            }
        }
        // Each pad-to-core path the input drives turns on its delay
        // setting (observed: 287 of 287 routed inputs; unrouted inputs 0),
        // with the third bit set when the path starts at the input register.
        for (k, &on) in u.datain_used.iter().enumerate() {
            if on {
                v.push((IO_SET_PAD_TO_CORE_DUAL_FUNC_DELAY_MAG, k as i32, if u.in_reg.is_some() { &[1, 1, 1] } else { &[1, 1, 0] }));
            }
        }
        // Registers: no clear (the USE_CLR bits read 1 then), clock from
        // the row clock buffer. The clock fast mux is one-hot, bit =
        // buffer index + 1 (observed for buffers 4 and 5).
        const ONE_HOT: [&[u8]; 6] = [
            &[0, 1, 0, 0, 0, 0, 0],
            &[0, 0, 1, 0, 0, 0, 0],
            &[0, 0, 0, 1, 0, 0, 0],
            &[0, 0, 0, 0, 1, 0, 0],
            &[0, 0, 0, 0, 0, 1, 0],
            &[0, 0, 0, 0, 0, 0, 1],
        ];
        let clk = |b: u32| ONE_HOT.get(b as usize).copied().with_context(|| format!("{}: I/O clock buffer {b}", u.pin));
        if let Some(b) = u.in_reg {
            v.push((IO_INREG_CLK_FAST_MUX, -1, clk(b)?));
            v.push((IO_USE_CLR_INREG, -1, &[1]));
        }
        if let Some(b) = u.out_reg {
            v.push((IO_OUTREG_CLK_FAST_MUX, -1, clk(b)?));
            v.push((IO_USE_CLR_OUTREG, -1, &[1]));
            v.push((IO_REG_FEEDS_PIN_OUT_YGR, -1, &[1, 0]));
        }
        if u.oe_reg {
            v.push((IO_USE_CLR_OEREG, -1, &[1]));
        }
    } else {
        ensure!(u.role == Role::In, "{}: dedicated clock pin used as {:?}", u.pin, u.role);
    }
    // Input buffer select: an output-only pin has it switched off the same
    // way an unused one does; 1.8 V and 1.5 V inputs use the low-voltage
    // buffer.
    v.push((
        IO_SELECT_IOST_DIRT,
        -1,
        match (u.role, u.standard) {
            (Role::Out, _) => &[1, 1, 0],
            (_, s) if s.low_voltage_input() => &[0, 1, 0],
            _ => &[0, 0, 0],
        },
    ));
    if u.pull_up {
        v.push((IO_USE_PULL_UP, -1, &[1]));
    }
    if u.bus_hold {
        v.push((IO_USE_BUS_HOLD, -1, &[1]));
    }
    Ok(v)
}

impl IoDb {
    /// Every IOE bit, for a design using `uses`. Unused dual-purpose pins
    /// are left out (they keep the base image).
    pub fn bits(&self, uses: &[PinUse], unused: UnusedPins) -> Result<Vec<(IoBit, bool)>> {
        let by_pin: HashMap<&str, &PinUse> = uses.iter().map(|u| (u.pin.as_str(), u)).collect();
        // A bank's I/O standard is that of the pins the design puts in it
        // (Quartus requires them to agree on VCCIO); empty banks are 3.3 V.
        let mut bank_std: HashMap<i32, Standard> = HashMap::new();
        for u in uses {
            let b = self.pins.get(&u.pin).with_context(|| format!("{} ({}) is not a user I/O pin", u.pin, u.port))?.bank;
            bank_std.insert(b, u.standard);
        }
        let mut out = Vec::new();
        let mut pins: Vec<&Pin> = self.pins.values().collect();
        pins.sort_by(|a, b| a.name.cmp(&b.name));
        for p in pins {
            let u = by_pin.get(p.name.as_str());
            let mut val: HashMap<IoBit, bool> = HashMap::new();
            for (_, bits) in self.footprint(p.ioe) {
                for b in bits.iter().flatten() {
                    val.insert(*b, false);
                }
            }
            let settings = match (u, p.dual_purpose) {
                (Some(u), _) => used_settings(p.ioe.block, u)?,
                (None, Some(d)) => {
                    dual_purpose_settings(p.ioe.block, d, bank_std.get(&p.bank).copied().unwrap_or(Standard::Lvttl33))?
                }
                (None, None) => unused_settings(p.ioe.block, unused),
            };
            for (a, i, v) in settings {
                let bits = self.feature(p.ioe, a, i)?;
                ensure!(bits.len() == v.len(), "{}: feature {a}/{i} has {} bits, setting {}", p.name, bits.len(), v.len());
                for (b, &x) in bits.iter().zip(v) {
                    if let Some(b) = b {
                        val.insert(*b, x == 1);
                    }
                }
            }
            if let Some(u) = u {
                for (a, line) in [(arch::IO_DATA_INPUT, u.data_line), (arch::IO_PIA_TO_OE, u.oe_line)] {
                    let Some(line) = line else { continue };
                    let m = self.muxes.get(&(p.ioe.block, a)).with_context(|| format!("{}: no source mux {a}", p.name))?;
                    for f in m.bits(p.ioe.path, line).with_context(|| format!("{}: line {line}", p.name))? {
                        let b = bit_of(f)?.context("source mux bit is a sentinel")?;
                        val.insert(b, true);
                    }
                }
            }
            out.extend(val);
        }
        out.sort();
        Ok(out)
    }

    /// LAB-local lines (of the IOE's output tile) the output data mux can
    /// select. Empty for blocks without one (dedicated clock inputs).
    pub fn data_sources(&self, p: &Pin) -> Vec<u32> {
        let Some(m) = self.muxes.get(&(p.ioe.block, arch::IO_DATA_INPUT)) else { return vec![] };
        (0..m.sel.len() as u32).filter(|&l| m.bits(p.ioe.path, l).is_ok()).collect()
    }

    /// Every bit any IOE owns (for residual accounting).
    pub fn all_bits(&self) -> impl Iterator<Item = IoBit> + '_ {
        self.features.values().flatten().flatten().flatten().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cff_lanes() {
        // Pull-up of the first row IOE (pad 0, E3) and of N2's IOE.
        let b = cff_bit(12446).unwrap();
        assert_eq!((b.frame, b.bit / 8, b.bit % 8), (5, 188, 0));
        let b = cff_bit(10908).unwrap();
        assert_eq!((b.frame, b.bit / 8, b.bit % 8), (0, 205, 0));
    }
}
