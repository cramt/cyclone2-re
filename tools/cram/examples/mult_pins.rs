//! Which physical data input (`MAC_ROW_DATA_INPUT` k) each logical
//! multiplier operand bit enters on: for every rcf sink `(cell, DATAA[i])`
//! at X39, the groups whose source mux for the sink's LAB line is set in
//! Quartus' RBF.
//!
//!     cargo run --release --example mult_pins <design dir>...
use chipsmith_cyclone2::bitstream::{addr, lab::Feature, le, rbf::Rbf};
use std::path::{Path, PathBuf};

fn main() -> anyhow::Result<()> {
    let devinfo = std::env::var_os("CRAM_DEVINFO").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap()).join("altera/13.0sp1/quartus/common/devinfo/cycloneii")
    });
    let m = cram::db::from_quartus(&devinfo)?;
    let a = &m.asmdb;
    let bi = a.block_info(41).unwrap();
    let base = |y: u32| -> u32 {
        let row = le::lab_row(y);
        let p = bi.block_lists.values().flatten().find(|&&p| p >> 16 == row).unwrap();
        row * bi.num_ram_cols as u32 + (p & 0xffff)
    };
    let mut groups: Vec<(i32, i32, Feature)> = a
        .arch_groups
        .iter()
        .filter(|g| g.block_type == 41 && [2139, 2034, 2031, 2032, 2033].contains(&g.arch_type))
        .map(|g| (g.arch_type, g.group_index, Feature::from(a, g)))
        .collect();
    groups.sort_by_key(|g| (g.0, g.1));
    for d in std::env::args().skip(1) {
        let dir = Path::new(&d);
        let rbf = Rbf::read(&dir.join("build/output_files/top.rbf"))?;
        let sigs = cram::rcf::read(&dir.join("build/top.rcf"))?;
        println!("== {d}");
        let mut lines = Vec::new();
        for s in &sigs {
            for dst in &s.dests {
                let Some(r) = dst.from.map(|i| &s.resources[i]) else { continue };
                if r.x != 39 {
                    continue;
                }
                // Control sinks hang off DSP_CONTROL_INPUT; use the line before.
                let (r, via) = if r.kind == "DSP_CONTROL_INPUT" {
                    let p = s.parents[dst.from.unwrap()].map(|p| &s.resources[p]).unwrap();
                    (p, format!(" via ctl {}", r.index))
                } else {
                    (r, String::new())
                };
                if r.kind != "LOCAL_INTERCONNECT" {
                    lines.push(format!("  {} {} Y{} <- {}:{}", dst.cell, dst.port, r.y, r.kind, r.index));
                    continue;
                }
                let b = base(r.y as u32) as i64;
                let hits: Vec<String> = groups
                    .iter()
                    .filter(|(_, _, f)| {
                        f.source_bits(0, r.index as usize)
                            .is_ok_and(|bits| bits.iter().all(|&o| rbf.get(addr::frame_bit((b + o as i64) as u32))))
                    })
                    .map(|(t, g, _)| format!("{t}/{g}"))
                    .collect();
                lines.push(format!("  {} {} Y{} line {}{via}: {}", dst.cell, dst.port, r.y, r.index, hits.join(" ")));
            }
        }
        lines.sort();
        for l in lines {
            println!("{l}");
        }
    }
    Ok(())
}
