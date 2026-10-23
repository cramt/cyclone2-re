//! Quartus frontend: a design dir after the oracle (build/top.rcf from
//! back-annotation, build/simulation/modelsim/top.vo from `just netlist`,
//! build/top.qsf) -> physical netlist.

use anyhow::{Context, Result};
use std::path::Path;

use ddb::device::Device;

use crate::logic::Logic;
use crate::phys::PhysDesign;
use crate::{io, m4k, rcf, route, vo};

pub struct Loaded {
    pub design: PhysDesign,
    /// Things the design uses that the model doesn't cover; their bits will
    /// be missing.
    pub unmodelled: Vec<String>,
    /// rcf resources that are not routing-graph edges (block-level hops).
    pub skipped_resources: usize,
}

pub fn load(dev: &Device, dir: &Path) -> Result<Loaded> {
    let sigs = rcf::read(&dir.join("build/top.rcf"))?;
    let (nets, skipped) = route::nets(dev, &sigs);
    let atoms = vo::read(&dir.join("build/simulation/modelsim/top.vo")).context("post-fit netlist (just netlist <dir>)")?;
    let (logic, unmodelled) = Logic::from_quartus(&atoms, &sigs)?;
    let qsf = std::fs::read_to_string(dir.join("build/top.qsf")).context("back-annotated qsf")?;
    let pins = io::pin_uses(&atoms, &sigs, &qsf)?;
    let unused_pins = io::UnusedPins::from_qsf(&qsf)?;
    let rcf_text = std::fs::read_to_string(dir.join("build/top.rcf"))?;
    let (m4ks, ram_unmodelled) = m4k::from_quartus(&atoms, &sigs, &rcf_text)?;
    let mut unmodelled = unmodelled;
    unmodelled.extend(ram_unmodelled);
    Ok(Loaded { design: PhysDesign { logic, pins, unused_pins, m4ks, nets }, unmodelled, skipped_resources: skipped.len() })
}
