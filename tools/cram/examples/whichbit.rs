//! Which asmdb arch groups name a raw bit value? (debug aid; LAB-relative
//! offsets aren't resolved, absolute-address groups like clock control are)
fn main() -> anyhow::Result<()> {
    let devinfo = std::path::PathBuf::from(std::env::var("HOME")?).join("altera/13.0sp1/quartus/common/devinfo/cycloneii");
    let db = ddb::asmdb::parse(&ddb::container::Ddb::open(&devinfo.join("ddb_magellan4_asmdb.ddb"))?.body)?;
    let want: Vec<i32> = std::env::args().skip(1).map(|a| a.parse().unwrap()).collect();
    for g in &db.arch_groups {
        for (pi, p) in g.paths().iter().enumerate() {
            for (li, m) in p.iter().enumerate() {
                let Some(m) = m else { continue };
                for (bi, b) in db.mux(*m).bits.iter().enumerate() {
                    if want.contains(b) {
                        println!("{b}: block {} arch {} group {} path {pi} level {li} bit {bi}", g.block_type, g.arch_type, g.group_index);
                    }
                }
            }
        }
        if let ddb::asmdb::ArchBody::Dumb { bits, .. } = &g.body {
            for (i, b) in bits.iter().enumerate() {
                if want.contains(b) {
                    println!("{b}: block {} arch {} group {} dumb index {i}", g.block_type, g.arch_type, g.group_index);
                }
            }
        }
    }
    for (i, b) in db.default_bits.iter().enumerate() {
        if want.contains(b) { println!("{b}: default_bits[{i}]"); }
    }
    Ok(())
}
