//! Attribute the bits a Quartus design sets beyond what cram models to the
//! multiplier block's asmdb features: for each residual bit, which block
//! (Y), arch group (name from archtype.json), group index, mux level and
//! position it is. The discovery tool of the multiplier RE.
//!
//!     cargo run --release --example mult_attr <archtype.json> <design dir>...
use chipsmith_cyclone2::bitstream::{self as bitstream, addr, blank, rbf::Rbf};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let names: Vec<String> = serde_json::from_str(&std::fs::read_to_string(&args[0])?)?;
    let devinfo = std::env::var_os("CRAM_DEVINFO").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap()).join("altera/13.0sp1/quartus/common/devinfo/cycloneii")
    });
    let m = cram::db::from_quartus(&devinfo)?;
    let a = &m.asmdb;
    let bi = a.block_info(41).expect("multiplier block info");
    let cols = bi.num_ram_cols as u32;
    // flat -> description, over every multiplier block.
    let mut owner: HashMap<u32, String> = HashMap::new();
    for &packed in bi.block_lists.values().flatten() {
        let (row, col) = (packed >> 16, packed & 0xffff);
        let y = 35 - (row - chipsmith_cyclone2::bitstream::le::lab_row(35)) / chipsmith_cyclone2::bitstream::le::LAB_ROWS;
        let base = (row * cols + col) as i64;
        for g in a.arch_groups.iter().filter(|g| g.block_type == 41) {
            let name = names.get(g.arch_type as usize).cloned().unwrap_or_else(|| g.arch_type.to_string());
            for (p, path) in g.paths().iter().enumerate() {
                for (l, mx) in path.iter().enumerate() {
                    let Some(mx) = mx else { continue };
                    for (k, &b) in a.mux(*mx).bits.iter().enumerate() {
                        if b < 0 {
                            continue;
                        }
                        owner.insert((base + b as i64) as u32, format!("Y{y} {name}[{}] p{p} L{l} #{k}", g.group_index));
                    }
                }
            }
        }
    }
    for d in &args[1..] {
        let dir = Path::new(d);
        let q = cram::quartus::load(&m.dev, dir)?;
        let (ours, _) = bitstream::assemble(&m, blank::image(&m.asmdb, &m.io)?, &q.design)?;
        let theirs = Rbf::read(&dir.join("build/output_files/top.rbf"))?;
        let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for b in ours.diff(&theirs) {
            if (b.frame as usize) < bitstream::rbf::FIRST_CRC_FRAME {
                by.entry("frames 0-23".into()).or_default().push(format!("{b}"));
                continue;
            }
            let Some(flat) = addr::flat(b) else {
                by.entry("unaddressable".into()).or_default().push(format!("{b}"));
                continue;
            };
            let v = u8::from(theirs.get(b));
            match owner.get(&flat) {
                Some(o) => by.entry(o.clone()).or_default().push(format!("={v}")),
                None => by.entry("unowned".into()).or_default().push(format!("{flat}({}/{})={v}", flat / addr::COLUMNS, flat % addr::COLUMNS)),
            }
        }
        println!("== {d}");
        for (k, v) in by {
            println!("  {k}: {}", v.join(" "));
        }
    }
    Ok(())
}
