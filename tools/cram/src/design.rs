//! What a placed-and-routed design configures, and the CRAM bits that
//! follow from it: routing from the rcf + ddb `asm`, LEs and LABs from the
//! post-fit netlist + rcf + ddb `asmdb` (`logic`), pins from the netlist,
//! qsf, rcf, ddb `pad`/`pkg` and the asmdb IOE groups (`io`).

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

use ddb::device::Device;

use crate::lab::LabDb;
use crate::logic::Logic;
use crate::io::{self, IoBit, IoDb};
use crate::rbf::{FrameBit, RepBit};
use crate::{addr, rcf, route, vo};

/// Where a bit's value came from, for checking and residual accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    /// ddb asm: a routing edge (value 1, or the encoded value).
    Routing,
    /// ddb asmdb: LUT contents, LE input muxes and LE/LAB settings.
    Logic,
    /// ddb pad/pkg/asmdb: I/O element configuration of every pin.
    Io,
}

pub struct Design {
    /// Every bit the model fixes, with its value. Later entries win, but
    /// the sources never overlap in practice (checked by `conflicts`).
    pub bits: Vec<(FrameBit, bool, Source)>,
    /// IOE bits in the replicated frames (`Source::Io`).
    pub rep: Vec<(RepBit, bool)>,
    pub logic: Logic,
    pub pins: Vec<io::PinUse>,
    pub skipped_resources: usize,
}

impl Design {
    /// A design dir after the oracle: build/top.rcf (back-annotated
    /// routing) and build/simulation/modelsim/top.vo (post-fit netlist).
    pub fn load(dev: &Device, lab: &LabDb, iodb: &IoDb, dir: &Path) -> Result<Self> {
        let sigs = rcf::read(&dir.join("build/top.rcf"))?;
        let (rbits, skipped) = route::bits(dev, &sigs)?;
        let mut bits: Vec<(FrameBit, bool, Source)> = rbits
            .iter()
            .map(|b| {
                // Encoded mux bits (GCLK -> row clock, ~1.5% of all) take
                // their listed level; plain ones are on when the edge is.
                let v = if b.use_encoded_setting != 0 { b.is_encoded_bit_high != 0 } else { true };
                (addr::frame_bit(b.flat_address), v, Source::Routing)
            })
            .collect();
        let atoms = vo::read(&dir.join("build/simulation/modelsim/top.vo")).context("post-fit netlist (just netlist <dir>)")?;
        let logic = Logic::build(&atoms, &sigs)?;
        for (flat, v) in logic.bits(lab)? {
            bits.push((addr::frame_bit(flat), v, Source::Logic));
        }
        let qsf = std::fs::read_to_string(dir.join("build/top.qsf")).context("back-annotated qsf")?;
        let pins = io::pin_uses(&atoms, &sigs, &qsf)?;
        let mut rep = Vec::new();
        for (b, v) in iodb.bits(&pins)? {
            match b {
                IoBit::Core(b) => bits.push((b, v, Source::Io)),
                IoBit::Rep(b) => rep.push((b, v)),
            }
        }
        Ok(Design { bits, rep, logic, pins, skipped_resources: skipped.len() })
    }

    /// Bits two sources set to different values.
    pub fn conflicts(&self) -> Vec<FrameBit> {
        let mut seen: HashMap<FrameBit, bool> = HashMap::new();
        let mut out = Vec::new();
        for &(b, v, _) in &self.bits {
            if let Some(&old) = seen.get(&b) {
                if old != v {
                    out.push(b);
                }
            }
            seen.insert(b, v);
        }
        out
    }
}

pub fn to_frame_bit(frame: u32, row: u32) -> FrameBit {
    addr::frame_bit(row * addr::COLUMNS + (addr::LAST_FRAME - frame))
}
