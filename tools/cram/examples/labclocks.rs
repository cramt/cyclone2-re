//! The row clock buffer each LAB clock of an RBF selects, for every LAB with
//! a clock: `x y clock0 clock1` (`-` unused). (exploration: which LAB clock
//! the fitter gives each clock domain)
use chipsmith_cyclone2::bitstream::{addr, asmdb::arch, lab::LabDb, rbf::Rbf};

fn main() -> anyhow::Result<()> {
    let devinfo = std::path::PathBuf::from(std::env::var("HOME")?)
        .join("altera/13.0sp1/quartus/common/devinfo/cycloneii");
    let db = ddb::asmdb::parse(&ddb::container::Ddb::open(&devinfo.join("ddb_magellan4_asmdb.ddb"))?.body)?;
    let lab = LabDb::new(&db)?;
    let r = Rbf::read(std::path::Path::new(&std::env::args().nth(1).unwrap()))?;
    for (x, y) in lab.lab_locs() {
        let mut out = Vec::new();
        for j in 0..2 {
            let f = lab.feature(arch::LAB_CLOCK_FAST_MUX, j)?;
            let bits = &f.paths[0][0];
            // Source 4 + b is row clock buffer b; its setting is the mux bit.
            let aux = f.aux.as_ref().unwrap();
            let mut got = "-".to_string();
            for b in 0..6 {
                let s = aux[4 + b] as usize;
                if r.get(addr::frame_bit(lab.flat(x, y, bits[s])?)) {
                    got = b.to_string();
                }
            }
            out.push(got);
        }
        if out.iter().any(|o| o != "-") {
            println!("{x} {y} {} {}", out[0], out[1]);
        }
    }
    Ok(())
}
