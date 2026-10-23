//! LAB and LE configuration bits, named and placed by the device database's
//! `asmdb` (block config tables), not by fuzzing. What fuzzing still supplies
//! is the *meaning* of a setting where the asmdb only gives an index (which
//! value of LE_CLOCK_SELECT is LAB clock 0, ...); those choices are the
//! `pub fn` docs below, each marked with how it was established.
//!
//! Geometry (asmdb `ASMDB_BLOCK_INFO`, observed to agree with the fuzz-fit
//! LUT model in `le`): a LAB block is 70 CRAM rows x 35 columns, its base
//! flat address at row `le::lab_row(y)` and the column of frame
//! `le::lut_frame(x) + 12`. Mux bits are flat offsets from that base.

use anyhow::{bail, ensure, Context, Result};
use std::collections::HashMap;

use ddb::asmdb::{arch, ArchGroup, Asmdb};

use crate::{addr, le};

/// One arch group of the LAB block type, flattened: per path (per LE for
/// LE-level groups), per mux level, the bit offsets; plus the source ->
/// setting maps of two-level muxes.
#[derive(Debug, Clone)]
pub struct Feature {
    pub paths: Vec<Vec<Vec<i32>>>,
    /// Source index -> first-level setting (-2: source not reachable).
    pub aux: Option<Vec<i32>>,
    /// Source index -> second-level setting.
    pub next: Option<Vec<i32>>,
}

impl Feature {
    pub fn from(db: &Asmdb, g: &ArchGroup) -> Self {
        let bits = |m: Option<ddb::asmdb::MuxId>| m.map(|m| db.mux(m).bits.clone()).unwrap_or_default();
        let paths = g.paths().iter().map(|p| p.iter().map(|&m| bits(m)).collect()).collect();
        let d = g.data().first();
        Feature {
            paths,
            aux: d.and_then(|d| d.aux).map(|m| db.mux(m).bits.clone()),
            next: d.and_then(|d| d.next_selection).map(|m| db.mux(m).bits.clone()),
        }
    }

    /// Offsets turned on to route `source` through the (one- or two-level)
    /// source mux of path `path`.
    pub fn source_bits(&self, path: usize, source: usize) -> Result<Vec<i32>> {
        let aux = self.aux.as_ref().context("not a source mux")?;
        let a = *aux.get(source).context("source out of range")?;
        ensure!(a >= 0, "source {source} not reachable (setting {a})");
        let levels = &self.paths[path];
        let mut out = vec![levels[0][a as usize]];
        if let (Some(next), Some(l1)) = (&self.next, levels.get(1)) {
            let n = next[source];
            if n >= 0 {
                out.push(l1[n as usize]);
            }
        }
        Ok(out)
    }

    /// The single bit of a one-bit-per-path group.
    pub fn bit(&self, path: usize) -> i32 {
        self.paths[path][0][0]
    }
}

pub struct LabDb {
    features: HashMap<(i32, i32), Feature>,
    /// Global clock control blocks, by G index: input-select bits (input
    /// index, LSB first) and the "in use" enable bit, absolute flat
    /// addresses (these groups have no block base).
    pub clkctrl_select: Vec<Vec<i32>>,
    pub clkctrl_enable: Vec<i32>,
    /// (x, y) -> base flat address of the LAB block.
    bases: HashMap<(u32, u32), u32>,
}

impl LabDb {
    pub fn new(db: &Asmdb) -> Result<Self> {
        let mut features = HashMap::new();
        for g in db.arch_groups.iter().filter(|g| g.block_type == arch::BLOCK_LAB) {
            features.insert((g.arch_type, g.group_index), Feature::from(db, g));
        }
        let bi = db.block_info(arch::BLOCK_LAB).context("asmdb has no LAB block info")?;
        let cols = bi.num_ram_cols as u32;
        ensure!(cols == addr::COLUMNS, "asmdb row width {cols} != {}", addr::COLUMNS);
        let frame_to_x: HashMap<u32, u32> = (1..=64).filter_map(|x| le::lut_frame(x).map(|f| (f, x))).collect();
        let mut bases = HashMap::new();
        for &packed in bi.block_lists.values().flatten() {
            let (row, col) = (packed >> 16, packed & 0xffff);
            let y = 35 - (row - le::lab_row(35)) / le::LAB_ROWS;
            let lut = addr::LAST_FRAME - col - 12;
            let Some(&x) = frame_to_x.get(&lut) else {
                bail!("asmdb LAB at row {row} column {col} matches no LAB column of the LUT model")
            };
            ensure!(le::lab_row(y) == row, "asmdb LAB row {row} is not a LAB row start");
            bases.insert((x, y), row * cols + col);
        }
        let one = |t: i32| -> Result<&ArchGroup> {
            let v: Vec<_> = db.arch_groups.iter().filter(|g| g.arch_type == t).collect();
            ensure!(v.len() == 1, "asmdb: {} groups of type {t}", v.len());
            Ok(v[0])
        };
        let sel = one(arch::CLKBUF_CLK_INPUT_SELECT_CYCLONEII)?;
        let clkctrl_select: Vec<Vec<i32>> =
            sel.paths().iter().map(|p| p.first().copied().flatten().map(|m| db.mux(m).bits.clone()).unwrap_or_default()).collect();
        let clkctrl_enable = match &one(arch::CLKBUF_ENABLE_MODE)?.body {
            ddb::asmdb::ArchBody::Dumb { bits, .. } => bits.clone(),
            _ => bail!("CLKBUF_ENABLE_MODE is not a plain bit list"),
        };
        ensure!(clkctrl_select.len() == 16 && clkctrl_enable.len() == 16, "expected 16 global clock controls");
        Ok(LabDb { features, bases, clkctrl_select, clkctrl_enable })
    }

    pub fn labs(&self) -> usize {
        self.bases.len()
    }

    /// (x, y) of every LAB, sorted.
    pub fn lab_locs(&self) -> Vec<(u32, u32)> {
        let mut v: Vec<_> = self.bases.keys().copied().collect();
        v.sort();
        v
    }

    pub fn feature(&self, arch_type: i32, group: i32) -> Result<&Feature> {
        self.features.get(&(arch_type, group)).with_context(|| format!("asmdb LAB group {arch_type}/{group} missing"))
    }

    /// Absolute flat address of offset `off` in LAB (x, y).
    pub fn flat(&self, x: u32, y: u32, off: i32) -> Result<u32> {
        let b = self.bases.get(&(x, y)).with_context(|| format!("no LAB at X{x} Y{y}"))?;
        Ok((*b as i64 + off as i64) as u32)
    }
}
