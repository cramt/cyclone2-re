//! What a placed-and-routed design configures, and the CRAM bits that
//! follows from it: routing from the rcf + asm, LUTs from LE placement.

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use ddb::device::Device;

use crate::rbf::FrameBit;
use crate::{addr, le, rcf, route};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LeLoc {
    pub x: u32,
    pub y: u32,
    pub n: u32,
}

/// One used logic element: its LUT plus the LE-level muxes we model.
#[derive(Debug, Clone)]
pub struct Le {
    pub loc: LeLoc,
    /// Truth table over *physical* inputs A + 2B + 4C + 8D.
    pub mask: u16,
    /// (LAB local line index, physical input 0..3) for each routed input.
    pub inputs: Vec<(u32, u32)>,
    /// Which of the LE's two LE_BUFFER outputs (rcf index n, n+1) drive a net.
    pub outputs: [bool; 2],
}

pub struct Design {
    pub routing: Vec<FrameBit>,
    pub les: Vec<Le>,
    pub skipped_resources: usize,
    /// LEs whose input net isn't in the rcf (first-generation fuzz design
    /// r1, before `keep`, renamed its cells): mask unknown, left out.
    pub unknown_luts: usize,
}

/// `set_location_assignment LCCOMB_X64_Y19_N0 -to <cell>` lines.
pub fn placements(qsf: &str) -> HashMap<String, LeLoc> {
    let mut m = HashMap::new();
    for line in qsf.lines() {
        let Some(rest) = line.trim().strip_prefix("set_location_assignment LCCOMB_X") else { continue };
        let Some((loc, cell)) = rest.split_once(" -to ") else { continue };
        let mut it = loc.split(['_', 'Y', 'N']).filter(|s| !s.is_empty()).map(str::parse::<u32>);
        if let (Some(Ok(x)), Some(Ok(y)), Some(Ok(n))) = (it.next(), it.next(), it.next()) {
            m.insert(cell.trim().trim_matches('"').to_string(), LeLoc { x, y, n });
        }
    }
    m
}

fn port_index(p: &str) -> Option<u32> {
    Some(match p {
        "DATAA" => 0,
        "DATAB" => 1,
        "DATAC" => 2,
        "DATAD" => 3,
        _ => return None,
    })
}

/// Physical truth table of a one-input buffer whose input lands on
/// physical input `i`.
fn buffer_mask(i: u32) -> u16 {
    (0..16).filter(|p| p >> i & 1 == 1).fold(0, |m, p| m | 1 << p)
}

impl Design {
    /// A fuzz design dir after `tools/route_fuzz.sh`: routing from
    /// build/top.rcf, LE placement from the back-annotated build/top.qsf.
    /// Every LE in these designs is a one-input buffer, so its physical
    /// mask follows from the `route_port` of the net driving it.
    pub fn from_fuzz_dir(dev: &Device, dir: &Path) -> Result<Self> {
        let sigs = rcf::read(&dir.join("build/top.rcf"))?;
        let (bits, skipped) = route::bits(dev, &sigs)?;
        let routing = bits.iter().map(|b| addr::frame_bit(b.flat_address)).collect();
        let qsf = std::fs::read_to_string(dir.join("build/top.qsf")).context("back-annotated qsf")?;
        let mut les = Vec::new();
        let mut unknown_luts = 0;
        for (cell, loc) in placements(&qsf) {
            let dests: Vec<(&rcf::Signal, &rcf::Dest)> =
                sigs.iter().flat_map(|s| s.dests.iter().map(move |d| (s, d))).filter(|(_, d)| d.cell == cell).collect();
            let Some(port) = dests.first().and_then(|(_, d)| port_index(d.physical_port())) else {
                unknown_luts += 1;
                continue;
            };
            let inputs = dests
                .iter()
                .filter_map(|(s, d)| {
                    let r = &s.resources[d.from?];
                    (r.kind == "LOCAL_INTERCONNECT").then(|| Some((r.index as u32, port_index(d.physical_port())?)))?
                })
                .collect();
            let mut outputs = [false; 2];
            for s in &sigs {
                if let Some(r) = s.resources.first().filter(|r| r.kind == "LE_BUFFER") {
                    let k = r.index as i64 - loc.n as i64;
                    if (r.x as u32, r.y as u32) == (loc.x, loc.y) && (0..2).contains(&k) {
                        outputs[k as usize] = true;
                    }
                }
            }
            les.push(Le { loc, mask: buffer_mask(port), inputs, outputs });
        }
        Ok(Design { routing, les, skipped_resources: skipped.len(), unknown_luts })
    }

    /// Non-routing bits this design sets to 1, by what sets them, relative
    /// to an empty design (all routing off, every LUT stored as 0 = mask
    /// 0xffff since LUT bits are stored inverted, no LE muxes on).
    pub fn le_ones(&self) -> (HashSet<FrameBit>, HashSet<FrameBit>) {
        let (mut lut, mut mux) = (HashSet::new(), HashSet::new());
        for l in &self.les {
            for p in 0..16 {
                if l.mask >> p & 1 == 0 {
                    lut.insert(lut_frame_bit(l.loc, p));
                }
            }
            for &(line, port) in &l.inputs {
                for (df, row) in le::input_mux(line, port).into_iter().flatten() {
                    mux.insert(le_frame_bit(l.loc, df, row));
                }
            }
            for (k, &on) in l.outputs.iter().enumerate() {
                if on {
                    let (df, row) = le::output_enable(k as u32);
                    mux.insert(le_frame_bit(l.loc, df, row));
                }
            }
        }
        (lut, mux)
    }

    /// Every bit position any used LUT owns, whatever its value.
    pub fn lut_bits(&self) -> HashSet<FrameBit> {
        self.les.iter().flat_map(|l| (0..16).map(move |p| lut_frame_bit(l.loc, p))).collect()
    }
}

fn to_frame_bit(frame: u32, row: u32) -> FrameBit {
    addr::frame_bit(row * addr::COLUMNS + (addr::LAST_FRAME - frame))
}

pub fn lut_frame_bit(loc: LeLoc, p: u32) -> FrameBit {
    let (frame, row) = le::lut_bit(loc.x, loc.y, loc.n, p).expect("placed LE outside the LAB grid");
    to_frame_bit(frame, row)
}

fn le_frame_bit(loc: LeLoc, df: i32, row: u32) -> FrameBit {
    let (frame, row) = le::le_bit(loc.x, loc.y, loc.n, df, row).expect("placed LE outside the LAB grid");
    to_frame_bit(frame, row)
}
