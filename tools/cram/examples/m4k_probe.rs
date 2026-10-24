//! What the chip database needs for an M4K tile: the routing elements at
//! each M4K block, and which LAB lines each input mux and control input
//! reaches (asmdb source maps).
//!
//!     cargo run --release --example m4k_probe
use ddb::asmdb::arch;
use std::collections::BTreeMap;

fn main() -> anyhow::Result<()> {
    let db = chipsmith_cyclone2::bitstream::phys::Db::load(&cram::db::locate()?)?;
    let dev = &db.dev;
    let r = &dev.routing;
    // Elements by (enum, z, index) at each M4K tile.
    let mut shapes: BTreeMap<Vec<(i16, i16, i16)>, Vec<(u32, u32)>> = BTreeMap::new();
    for (x, y) in db.m4k.locs() {
        let mut v = Vec::new();
        for (i, e) in r.elements.iter().enumerate() {
            if (e.x as u32, e.y as u32) == (x, y) {
                let t = &r.templates[e.template as usize];
                let _ = i;
                v.push((t.element_enum, t.z, t.index));
            }
        }
        v.sort();
        shapes.entry(v).or_default().push((x, y));
    }
    for (s, locs) in &shapes {
        let mut by: BTreeMap<i16, Vec<(i16, i16)>> = BTreeMap::new();
        for &(e, z, i) in s {
            by.entry(e).or_default().push((z, i));
        }
        println!("{} tiles e.g. {:?}", locs.len(), &locs[..locs.len().min(4)]);
        for (e, v) in by {
            println!("  {} ({e}): {} elements, z/index {:?}..{:?}", chipsmith_cyclone2::bitstream::device::element_name(e), v.len(), v.first(), v.last());
        }
    }
    let show = |name: &str, t: i32, idx: &[i32]| -> anyhow::Result<()> {
        for &i in idx {
            let f = db.m4k.feature(t, i)?;
            let aux = f.aux.as_ref();
            let reach: Vec<usize> = match aux {
                Some(a) => (0..a.len()).filter(|&s| a[s] >= 0 && f.source_bits(0, s).is_ok()).collect(),
                None => vec![],
            };
            println!("{name} {i}: paths {} levels {:?} aux {:?} reach {reach:?}", f.paths.len(), f.paths[0].iter().map(|l| l.len()).collect::<Vec<_>>(), aux.map(|a| a.len()));
        }
        Ok(())
    };
    show("DATA_INPUT", arch::RAM_MS_DATA_INPUT, &(0..36).collect::<Vec<_>>())?;
    show("WRITE_ADDR", arch::RAM_MS_WRITE_ADDRESS_INPUT, &(0..12).filter(|&i| db.m4k.feature(arch::RAM_MS_WRITE_ADDRESS_INPUT, i).is_ok()).collect::<Vec<_>>())?;
    show("READ_ADDR", arch::RAM_MS_READ_ADDRESS_INPUT, &(0..12).filter(|&i| db.m4k.feature(arch::RAM_MS_READ_ADDRESS_INPUT, i).is_ok()).collect::<Vec<_>>())?;
    show("BYTE_ENA", arch::RAM_MS_BYTE_ENABLE_INPUT, &(0..4).filter(|&i| db.m4k.feature(arch::RAM_MS_BYTE_ENABLE_INPUT, i).is_ok()).collect::<Vec<_>>())?;
    show("PIA", arch::RAM_PIA_TO_INPUT_YGR, &(0..8).collect::<Vec<_>>())?;
    for (n, t, idx) in [
        ("CLOCK", arch::RAM_CLOCK_FAST_MUX, vec![0, 1]),
        ("CE", arch::RAM_CE_FAST_MUX, vec![0, 1]),
        ("CLEAR", arch::RAM_CLEAR_FAST_MUX, vec![0, 1]),
        ("WE", arch::RAM_WE_FAST_MUX, vec![-1]),
        ("RE", arch::RAM_RE_FAST_MUX, vec![-1]),
    ] {
        for i in idx {
            let f = db.m4k.feature(t, i)?;
            println!("{n} {i}: aux {:?} bits {:?}", f.aux, f.paths[0][0].len());
        }
    }
    Ok(())
}
