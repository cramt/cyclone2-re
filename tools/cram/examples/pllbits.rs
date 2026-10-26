//! Every PLL feature's bits as an RBF holds them, for the PLL instances that
//! differ from the empty image: one line per (instance, arch group, group
//! index, path, mux level), bits in the mux's order. (exploration)
//!
//!     cargo run --release --example pllbits -- empty.rbf design.rbf...
use chipsmith_cyclone2::bitstream::{addr, io::cff_bit, rbf::Rbf};

fn main() -> anyhow::Result<()> {
    let devinfo = std::path::PathBuf::from(std::env::var("HOME")?)
        .join("altera/13.0sp1/quartus/common/devinfo/cycloneii");
    let db = ddb::asmdb::parse(&ddb::container::Ddb::open(&devinfo.join("ddb_magellan4_asmdb.ddb"))?.body)?;
    let a: Vec<String> = std::env::args().skip(1).collect();
    let empty = Rbf::read(std::path::Path::new(&a[0]))?;
    let bases: Vec<u32> = db.block_info(43).unwrap().block_lists.values().flatten().map(|&p| p & 0xffff).collect();
    for f in &a[1..] {
        let r = Rbf::read(std::path::Path::new(f))?;
        for (inst, &base) in bases.iter().enumerate() {
            let get = |r: &Rbf, b: i32| -> Option<bool> {
                if b >= 0 {
                    Some(r.get(addr::frame_bit(b as u32)))
                } else if (b as u32) >> 24 == 0xa0 {
                    Some(r.rep_get(cff_bit(base + (b as u32 & 0xff_ffff)).ok()?))
                } else {
                    None
                }
            };
            let mut lines = Vec::new();
            let mut differs = false;
            for g in db.arch_groups.iter().filter(|g| g.block_type == 43) {
                for (pi, p) in g.paths().iter().enumerate() {
                    // Core-address groups list one path per PLL instance.
                    if g.paths().len() == 4 && pi != inst {
                        continue;
                    }
                    for (li, m) in p.iter().enumerate() {
                        let Some(m) = m else { continue };
                        let bits = &db.mux(*m).bits;
                        let v: String = bits.iter().map(|&b| match get(&r, b) { Some(true) => '1', Some(false) => '0', None => '.' }).collect();
                        let e: String = bits.iter().map(|&b| match get(&empty, b) { Some(true) => '1', Some(false) => '0', None => '.' }).collect();
                        differs |= v != e;
                        lines.push(format!("{f} pll{inst} arch{} grp{} path{pi} lvl{li} {v}{}", g.arch_type, g.group_index, if v != e { " *" } else { "" }));
                    }
                }
            }
            if differs {
                for l in lines {
                    println!("{l}");
                }
            }
        }
    }
    Ok(())
}
