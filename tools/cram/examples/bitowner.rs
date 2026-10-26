//! Every bit two RBFs differ in, with what the device database says owns it:
//! asmdb features (block, arch type, group, path, mux level, bit index;
//! block instances resolved), routing edges, or nothing. Replicated-frame
//! (CFF) bits are attributed through their CFF index. (exploration)
//!
//!     cargo run --release --example bitowner -- ours.rbf theirs.rbf
use chipsmith_cyclone2::bitstream::{addr, io::cff_bit, rbf::Rbf};
use std::collections::HashMap;

fn main() -> anyhow::Result<()> {
    let devinfo = std::path::PathBuf::from(std::env::var("HOME")?)
        .join("altera/13.0sp1/quartus/common/devinfo/cycloneii");
    let a: Vec<String> = std::env::args().skip(1).collect();
    let ours = Rbf::read(std::path::Path::new(&a[0]))?;
    let theirs = Rbf::read(std::path::Path::new(&a[1]))?;
    let db = ddb::asmdb::parse(&ddb::container::Ddb::open(&devinfo.join("ddb_magellan4_asmdb.ddb"))?.body)?;
    let dev = ddb::device::load(&devinfo, "magellan4")?;
    // flat or CFF (0x80000000 | k) -> owners
    let mut owner: HashMap<u32, Vec<String>> = HashMap::new();
    for g in &db.arch_groups {
        let bases: Vec<u32> = match db.block_info(g.block_type) {
            Some(bi) => bi
                .block_lists
                .values()
                .flatten()
                .map(|&p| (p >> 16) * bi.num_ram_cols as u32 + (p & 0xffff))
                .collect(),
            None => vec![0],
        };
        let key = |b: i32, base: u32| -> Option<u32> {
            if b >= 0 {
                Some(if g.block_type == 43 || bases.len() == 1 && bases[0] == 0 { b as u32 } else { base + b as u32 })
            } else if (b as u32) >> 24 == 0xa0 {
                Some(0x8000_0000 | (base + (b as u32 & 0xff_ffff)))
            } else {
                None
            }
        };
        for (pi, p) in g.paths().iter().enumerate() {
            for (li, m) in p.iter().enumerate() {
                let Some(m) = m else { continue };
                for (bi, &b) in db.mux(*m).bits.iter().enumerate() {
                    for (k, &base) in bases.iter().enumerate() {
                        if let Some(f) = key(b, base) {
                            owner.entry(f).or_default().push(format!(
                                "blk{} arch{} grp{} path{pi} inst{k} lvl{li} bit{bi}",
                                g.block_type, g.arch_type, g.group_index
                            ));
                        }
                    }
                }
            }
        }
        if let ddb::asmdb::ArchBody::Dumb { bits, .. } = &g.body {
            for (i, &b) in bits.iter().enumerate() {
                if let Some(f) = key(b, 0) {
                    owner.entry(f).or_default().push(format!(
                        "blk{} arch{} grp{} dumb{i}",
                        g.block_type, g.arch_type, g.group_index
                    ));
                }
            }
        }
    }
    for (n, asm) in dev.asm.iter().enumerate() {
        let t = &dev.routing.templates[dev.routing.elements[n].template as usize];
        for gi in 0..asm.start_bit_index.len() {
            for b in asm.group(gi) {
                owner.entry(b.flat_address).or_default().push(format!(
                    "route E{} X{}Y{}S{}I{} edge{gi}",
                    t.element_enum, dev.routing.elements[n].x, dev.routing.elements[n].y, t.z, t.index
                ));
            }
        }
    }
    let mut cff: HashMap<_, u32> = HashMap::new();
    for k in 0..12_456 {
        if let Ok(b) = cff_bit(k) {
            cff.insert(b, k);
        }
    }
    for b in ours.rep_diff(&theirs) {
        let k = cff.get(&b).copied();
        let o = k.and_then(|k| owner.get(&(0x8000_0000 | k)));
        println!(
            "rep {b} cff {k:?} ours={} quartus={} {:?}",
            u8::from(ours.rep_get(b)),
            u8::from(theirs.rep_get(b)),
            o
        );
    }
    for b in ours.diff(&theirs) {
        let f = addr::flat(b);
        let o = f.and_then(|f| owner.get(&f));
        println!(
            "{b} flat {f:?} ours={} quartus={} {:?}",
            u8::from(ours.get(b)),
            u8::from(theirs.get(b)),
            o
        );
    }
    Ok(())
}
