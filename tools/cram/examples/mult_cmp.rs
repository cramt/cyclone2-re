//! The multiplier blocks of a Quartus build (post-fit atoms + rcf) beside
//! chipsmith's of the same design (routed.json), block by block: where a
//! byte diff in the multiplier column comes from.
//!
//!     cargo run --release --example mult_cmp <quartus design dir> <routed.json>
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let m = chipsmith_cyclone2::bitstream::phys::Db::load(&cram::db::locate()?)?;
    let q = cram::quartus::load(&m.dev, Path::new(&args[0]))?;
    let routed = chipsmith_cyclone2::bitstream::nextpnr::read(&m.dev, Path::new(&args[1]))?;
    let ours = routed.to_phys(&m.dev, &m.io)?;
    let mut qm = q.design.mults.clone();
    qm.sort_by_key(|b| b.y);
    let mut om = ours.mults.clone();
    om.sort_by_key(|b| b.y);
    for (a, b) in qm.iter().zip(&om) {
        println!("Y{}: {}", a.y, if a == b { "same" } else { "DIFFER" });
        if a != b {
            println!("  quartus {a:?}\n  ours    {b:?}");
        }
    }
    Ok(())
}
