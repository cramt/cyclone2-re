//! Quartus frontend: a design dir after the oracle (build/top.rcf from
//! back-annotation, build/simulation/modelsim/top.vo from `just netlist`,
//! build/top.qsf) -> physical netlist.

use anyhow::{Context, Result};
use std::path::Path;

use chipsmith_cyclone2::bitstream::device::Device;
use chipsmith_cyclone2::bitstream::phys::PhysDesign;

use crate::{rcf, route, vo};

pub mod export;
pub mod hard;
pub mod io;
pub mod logic;
pub mod m4k;
pub mod pll;

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
    let (logic, unmodelled) = logic::from_atoms(&atoms, &sigs)?;
    let qsf = std::fs::read_to_string(dir.join("build/top.qsf")).context("back-annotated qsf")?;
    let pins = io::pin_uses(&atoms, &sigs, &qsf)?;
    let unused_pins = io::unused_pins(&qsf)?;
    let rcf_text = std::fs::read_to_string(dir.join("build/top.rcf"))?;
    let (m4ks, ram_unmodelled) = m4k::from_atoms(&atoms, &sigs, &rcf_text)?;
    let hard = hard::from_atoms(&atoms, &sigs)?;
    let (plls, pll_unmodelled) = pll::from_atoms(&atoms, &sigs)?;
    let mut unmodelled = unmodelled;
    unmodelled.extend(ram_unmodelled);
    unmodelled.extend(pll_unmodelled);
    Ok(Loaded { design: PhysDesign { logic, pins, unused_pins, m4ks, hard, plls, nets }, unmodelled, skipped_resources: skipped.len() })
}
