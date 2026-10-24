//! The JTAG and serial-flash (ASMI) hard blocks: what a design connects to
//! them, and the CRAM bits that follow. A design that uses both can drive
//! the configuration flash (EPCS) from the JTAG cable, which is what a
//! serial flash loader is.
//!
//! Device side (observed, README "JTAG and serial-flash blocks"): both
//! blocks sit in the non-LAB column X6, the JTAG block at Y21-22 (z 0), the
//! serial-flash block at Y20 (z 2). Their outputs are routing elements that
//! drive R4/C4/local lines like an LE output (enums 361-368, 474). Their
//! inputs are asmdb source muxes over the 15 LOCAL_INTERCONNECT lines of
//! their tile: block type 9 (`JTAG_PIA_TO_TDOUSER`, X6 Y22) and block type
//! 15 (`SPI_PIA_TO_DCLKO`/`_SCE`/`_SDO`, X6 Y20). Source `s` of a mux is
//! line `s`. Using the serial-flash block also sets three bits outside the
//! block: option bit 0 of header byte 0x60 and the pull-ups of the ASDO and
//! nCSO pins go off (the block drives them).

use anyhow::{bail, ensure, Context, Result};

use ddb::asmdb::Asmdb;

use crate::lab::Feature;
use crate::rcf::Signal;
use crate::vo::Atom;

/// Header ID-block option bit the serial-flash block sets: (byte, bit).
pub const ASMI_OPTION: (usize, u8) = (0x60, 0);

/// Where the blocks are, in Quartus tile coordinates.
pub const JTAG_XY: (u32, u32) = (6, 22);
pub const ASMI_XY: (u32, u32) = (6, 20);
/// LOCAL_INTERCONNECT lines in each block's tile.
pub const LINES: usize = 15;

/// asmdb block types and arch types of the input muxes (libdb_asmdb.so's
/// translator table names them).
const BLOCK_JTAG: i32 = 9;
const BLOCK_SPI: i32 = 15;
const JTAG_PIA_TO_TDOUSER: i32 = 519;
const SPI_PIA_TO_SCE: i32 = 544;
const SPI_PIA_TO_SDO: i32 = 546;
const SPI_PIA_TO_DCLKO: i32 = 548;

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

/// A local line of the block's tile (0..15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Line(u8);

impl Line {
    pub fn new(l: u32) -> Result<Self> {
        ensure!((l as usize) < LINES, "local line {l} out of range");
        Ok(Line(l as u8))
    }
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// The JTAG block in use. Its outputs need no configuration; only what
/// drives TDO during a user instruction does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jtag {
    /// The line `tdouser` comes in on; `None` leaves it undriven.
    pub tdouser: Option<Line>,
}

/// The serial-flash block in use: DCLK, nCSO and ASDO driven from the
/// core, DATA0 read back. Its output enable (`oe`, active low) is tied
/// low, the only setting modelled, so the block always drives the pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asmi {
    pub dclk: Line,
    pub sce: Line,
    pub sdo: Line,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hard {
    pub jtag: Option<Jtag>,
    pub asmi: Option<Asmi>,
}

pub struct HardDb {
    tdouser: Feature,
    dclk: Feature,
    sce: Feature,
    sdo: Feature,
}

impl HardDb {
    pub fn new(db: &Asmdb) -> Result<Self> {
        let feature = |block: i32, arch: i32| -> Result<Feature> {
            let g = db
                .arch_groups
                .iter()
                .find(|g| g.block_type == block && g.arch_type == arch)
                .with_context(|| format!("asmdb has no group {arch} in block type {block}"))?;
            Ok(Feature::from(db, g))
        };
        Ok(HardDb {
            tdouser: feature(BLOCK_JTAG, JTAG_PIA_TO_TDOUSER)?,
            dclk: feature(BLOCK_SPI, SPI_PIA_TO_DCLKO)?,
            sce: feature(BLOCK_SPI, SPI_PIA_TO_SCE)?,
            sdo: feature(BLOCK_SPI, SPI_PIA_TO_SDO)?,
        })
    }

    /// Lines each input's mux can select.
    pub fn sources(&self, pin: HardInput) -> Vec<Line> {
        let f = self.feature(pin);
        (0..LINES).filter(|&s| f.source_bits(0, s).is_ok()).map(|s| Line(s as u8)).collect()
    }

    fn feature(&self, pin: HardInput) -> &Feature {
        match pin {
            HardInput::Tdouser => &self.tdouser,
            HardInput::Dclk => &self.dclk,
            HardInput::Sce => &self.sce,
            HardInput::Sdo => &self.sdo,
        }
    }

    /// Flat CRAM addresses set to 1. The groups hold absolute addresses
    /// (no block base), like the clock controls'.
    pub fn bits(&self, h: &Hard) -> Result<Vec<u32>> {
        let mut used = Vec::new();
        if let Some(j) = &h.jtag {
            used.extend(j.tdouser.map(|l| (HardInput::Tdouser, l)));
        }
        if let Some(a) = &h.asmi {
            used.extend([(HardInput::Dclk, a.dclk), (HardInput::Sce, a.sce), (HardInput::Sdo, a.sdo)]);
        }
        let mut out = Vec::new();
        for (pin, line) in used {
            for b in self.feature(pin).source_bits(0, line.index()).with_context(|| format!("{pin:?} from line {}", line.0))? {
                ensure!(b >= 0, "{pin:?}: sentinel mux bit");
                out.push(b as u32);
            }
        }
        Ok(out)
    }
}

/// An input of a hard block that a design can route a net into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HardInput {
    Tdouser,
    Dclk,
    Sce,
    Sdo,
}

impl HardInput {
    pub const ALL: [HardInput; 4] = [HardInput::Tdouser, HardInput::Dclk, HardInput::Sce, HardInput::Sdo];
    /// The chip database's bel and pin (`chipdb.rs`, nextpnr JSON).
    pub fn bel_pin(self) -> (&'static str, &'static str) {
        match self {
            HardInput::Tdouser => ("JTAG", "TDOUSER"),
            HardInput::Dclk => ("ASMI", "DCLKIN"),
            HardInput::Sce => ("ASMI", "SCEIN"),
            HardInput::Sdo => ("ASMI", "SDOIN"),
        }
    }
    /// The rcf resource the input mux drives.
    fn rcf_kind(self) -> &'static str {
        match self {
            HardInput::Tdouser => "JTAG_TDOUSER_PIN",
            HardInput::Dclk => "ASMI_DCLKIN",
            HardInput::Sce => "ASMI_SCEIN",
            HardInput::Sdo => "ASMI_SDOIN",
        }
    }
    pub fn xy(self) -> (u32, u32) {
        match self {
            HardInput::Tdouser => JTAG_XY,
            _ => ASMI_XY,
        }
    }
}

/// The hard blocks of a Quartus post-fit netlist and the lines its rcf
/// routes into them.
pub fn from_quartus(atoms: &[Atom], sigs: &[Signal]) -> Result<Hard> {
    let line = |pin: HardInput| -> Result<Option<Line>> {
        for s in sigs {
            for (i, r) in s.resources.iter().enumerate() {
                if r.kind != pin.rcf_kind() {
                    continue;
                }
                let p = s.parents[i].map(|p| &s.resources[p]).filter(|p| p.kind == "LOCAL_INTERCONNECT");
                let p = p.with_context(|| format!("{}: {} not fed by a local line", s.name, r.kind))?;
                ensure!(
                    (p.x as u32, p.y as u32) == pin.xy(),
                    "{}: {} fed from X{} Y{}",
                    s.name,
                    r.kind,
                    p.x,
                    p.y
                );
                return Line::new(p.index as u32).map(Some);
            }
        }
        Ok(None)
    };
    let mut h = Hard::default();
    for a in atoms {
        match a.kind.as_str() {
            "jtag" => {
                ensure!(h.jtag.is_none(), "two JTAG blocks");
                h.jtag = Some(Jtag { tdouser: line(HardInput::Tdouser)? });
            }
            "asmiblock" => {
                ensure!(h.asmi.is_none(), "two serial-flash blocks");
                ensure!(a.port("oe") == Some("gnd"), "{}: only oe tied low is modelled", a.name);
                let need = |pin| line(pin)?.with_context(|| format!("{}: {pin:?} not routed", a.name));
                h.asmi = Some(Asmi { dclk: need(HardInput::Dclk)?, sce: need(HardInput::Sce)?, sdo: need(HardInput::Sdo)? });
            }
            "crcblock" => bail!("{}: the CRC block isn't modelled", a.name),
            _ => {}
        }
    }
    Ok(h)
}
