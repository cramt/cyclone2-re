//! What the multiplier blocks (asmdb block type 41) configure and which
//! routing elements sit at their tiles: the starting point of the
//! multiplier RE.
//!
//!     cargo run --release --example mult_probe [archtype.json]
use std::collections::BTreeMap;

fn main() -> anyhow::Result<()> {
    let db = chipsmith_cyclone2::bitstream::phys::Db::load(&cram::db::locate()?)?;
    let names: Vec<String> = match std::env::args().nth(1) {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(p)?)?,
        None => vec![],
    };
    let a = &db.asmdb;
    let bi = a.block_info(41).expect("block type 41");
    println!("block lists {:?}", bi.block_lists.iter().map(|(k, v)| (k, v.len(), v.iter().take(3).map(|b| (b >> 16, b & 0xffff)).collect::<Vec<_>>())).collect::<Vec<_>>());
    println!("simple {:?}", bi.simple_block_lists.iter().map(|(k, v)| (k, v >> 16, v & 0xffff)).collect::<Vec<_>>());
    let mut by: BTreeMap<i32, Vec<&chipsmith_cyclone2::bitstream::asmdb::ArchGroup>> = BTreeMap::new();
    for g in a.arch_groups.iter().filter(|g| g.block_type == 41) {
        by.entry(g.arch_type).or_default().push(g);
    }
    for (t, gs) in by {
        let name = names.get(t as usize).cloned().unwrap_or_default();
        let idx: Vec<i32> = gs.iter().map(|g| g.group_index).collect();
        let g = gs[0];
        let shape = match &g.body {
            chipsmith_cyclone2::bitstream::asmdb::ArchBody::Muxed { paths, data, .. } => {
                let lv: Vec<String> = paths[0].iter().map(|m| m.map(|m| format!("{:?}{:?}", a.mux(m).kind, a.mux(m).bits.len())).unwrap_or("-".into())).collect();
                format!("muxed paths {} levels {lv:?} data {}", paths.len(), data.len())
            }
            chipsmith_cyclone2::bitstream::asmdb::ArchBody::Dumb { bits, .. } => format!("dumb {bits:?}"),
            chipsmith_cyclone2::bitstream::asmdb::ArchBody::Stlmap { paths, data } => format!("stlmap paths {} data {}", paths.len(), data.len()),
        };
        println!("{t} {name} idx {idx:?} {shape}");
    }
    // Routing elements at X39.
    let r = &db.dev.routing;
    let mut els: BTreeMap<(i16, i16), Vec<(u32, i16)>> = BTreeMap::new();
    for e in r.elements.iter() {
        if e.x == 39 && e.y == 5 {
            let t = &r.templates[e.template as usize];
            els.entry((t.element_enum, t.z)).or_default().push((e.y as u32, t.index));
        }
    }
    for ((en, z), v) in els {
        println!("X39Y5 {} ({en}) z{z}: {} idx {:?}", chipsmith_cyclone2::bitstream::device::element_name(en), v.len(), v.iter().map(|x| x.1).collect::<Vec<_>>());
    }
    Ok(())
}
