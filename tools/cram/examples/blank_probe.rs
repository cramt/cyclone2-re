//! Which asmdb features hold the empty image's set bits? (exploration)
use chipsmith_cyclone2::bitstream::rbf::{FrameBit, Rbf};
use std::collections::{BTreeMap, HashMap, HashSet};
fn main() -> anyhow::Result<()> {
    let devinfo = std::path::PathBuf::from(std::env::var("HOME")?).join("altera/13.0sp1/quartus/common/devinfo/cycloneii");
    let db = ddb::asmdb::parse(&ddb::container::Ddb::open(&devinfo.join("ddb_magellan4_asmdb.ddb"))?.body)?;
    let io = chipsmith_cyclone2::bitstream::io::IoDb::new(&cram::db::pins_from_quartus(&devinfo)?, &db)?;
    let empty = Rbf::read(std::path::Path::new(&std::env::args().nth(1).unwrap()))?;
    // flat -> (block type, arch, group, path, level, bit index)
    let mut owner: HashMap<u32, Vec<(i32, i32, i32, usize, usize, usize)>> = HashMap::new();
    let mut instances: BTreeMap<i32, usize> = BTreeMap::new();
    for g in &db.arch_groups {
        let bases: Vec<u32> = match db.block_info(g.block_type) {
            Some(bi) => bi.block_lists.values().flatten().map(|&p| (p >> 16) * bi.num_ram_cols as u32 + (p & 0xffff)).collect(),
            None => vec![0],
        };
        instances.insert(g.block_type, bases.len());
        for (pi, p) in g.paths().iter().enumerate() {
            for (li, m) in p.iter().enumerate() {
                let Some(m) = m else { continue };
                for (bi, &b) in db.mux(*m).bits.iter().enumerate() {
                    if b < 0 { continue }
                    for (k, &base) in bases.iter().enumerate() {
                        let pi = if g.block_type == 43 { pi * 100 + k } else { pi };
                        owner.entry((base as i64 + b as i64) as u32).or_default().push((g.block_type, g.arch_type, g.group_index, pi, li, bi));
                    }
                }
            }
        }
        if let ddb::asmdb::ArchBody::Dumb { bits, .. } = &g.body {
            for (i, &b) in bits.iter().enumerate() {
                if b >= 0 { owner.entry(b as u32).or_default().push((g.block_type, g.arch_type, g.group_index, i, 0, 0)); }
            }
        }
    }
    let defaults: HashSet<u32> = db.default_bits.iter().map(|&b| b as u32).collect();
    let iob: HashSet<_> = io.all_bits().collect();
    let mut by: BTreeMap<(i32, i32, i32, usize, usize, usize), usize> = BTreeMap::new();
    let mut nobody = 0; let mut nobody_frames: BTreeMap<u16, usize> = BTreeMap::new();
    for f in chipsmith_cyclone2::bitstream::rbf::FIRST_CRC_FRAME..chipsmith_cyclone2::bitstream::rbf::FRAMES {
        for bit in 0..chipsmith_cyclone2::bitstream::rbf::FRAME_BITS {
            let b = FrameBit { frame: f as u16, bit: bit as u16 };
            if !empty.get(b) || iob.contains(&chipsmith_cyclone2::bitstream::io::IoBit::Core(b)) { continue }
            let Some(flat) = chipsmith_cyclone2::bitstream::addr::flat(b) else { continue };
            if defaults.contains(&flat) { continue }
            match owner.get(&flat) {
                Some(v) => { for o in v { *by.entry(*o).or_default() += 1; } }
                None => { nobody += 1; *nobody_frames.entry(f as u16).or_default() += 1; }
            }
        }
    }
    println!("instances per block type: {instances:?}");
    for (k, n) in &by { println!("{k:?}: {n}"); }
    println!("set bits owned by no feature: {nobody} {nobody_frames:?}");
    Ok(())
}
