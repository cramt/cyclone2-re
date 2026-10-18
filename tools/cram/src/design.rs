//! What a placed-and-routed design configures, and the CRAM bits that
//! follow from it: routing from the rcf + ddb `asm`, LEs and LABs from the
//! post-fit netlist + rcf + ddb `asmdb` (`logic`), output-pin data muxes
//! from a learned table.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

use ddb::device::Device;

use crate::lab::LabDb;
use crate::logic::Logic;
use crate::rbf::FrameBit;
use crate::{addr, le, rcf, route, vo};

/// An output pin's data input, in a bottom-row I/O tile: (x, z, LAB-local
/// line feeding IO_DATAOUT).
#[derive(Debug, Clone, Copy)]
pub struct IoOut {
    pub x: u32,
    pub z: u32,
    pub line: u32,
}

/// Where a bit's value came from, for checking and residual accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    /// ddb asm: a routing edge (value 1, or the encoded value).
    Routing,
    /// ddb asmdb: LUT contents, LE input muxes and LE/LAB settings.
    Logic,
    /// Learned table: bottom-row output pin data mux.
    IoOutMux,
}

pub struct Design {
    /// Every bit the model fixes, with its value. Later entries win, but
    /// the sources never overlap in practice (checked by `conflicts`).
    pub bits: Vec<(FrameBit, bool, Source)>,
    pub logic: Logic,
    pub io_outs: Vec<IoOut>,
    /// Output pins whose data mux isn't modelled (not bottom row, or a
    /// line the learned table hasn't seen).
    pub unmodelled_io_outs: usize,
    pub skipped_resources: usize,
}

impl Design {
    /// A design dir after the oracle: build/top.rcf (back-annotated
    /// routing) and build/simulation/modelsim/top.vo (post-fit netlist).
    pub fn load(dev: &Device, lab: &LabDb, dir: &Path) -> Result<Self> {
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
        let (mut io_outs, mut unmodelled_io_outs) = (Vec::new(), 0);
        for s in &sigs {
            for (i, r) in s.resources.iter().enumerate().filter(|(_, r)| r.kind == "IO_DATAOUT") {
                match i.checked_sub(1).map(|j| &s.resources[j]) {
                    Some(l) if l.kind == "LOCAL_INTERCONNECT" && r.y == 0 => {
                        let o = IoOut { x: r.x as u32, z: r.z as u32, line: l.index as u32 };
                        match le::io_out_mux(o.x, o.z, o.line) {
                            Some(m) => {
                                for (frame, row) in m {
                                    bits.push((to_frame_bit(frame, row), true, Source::IoOutMux));
                                }
                                io_outs.push(o);
                            }
                            None => unmodelled_io_outs += 1,
                        }
                    }
                    _ => unmodelled_io_outs += 1,
                }
            }
        }
        Ok(Design { bits, logic, io_outs, unmodelled_io_outs, skipped_resources: skipped.len() })
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
