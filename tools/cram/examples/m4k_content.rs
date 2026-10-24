//! The contents of one M4K block in an RBF, as rows of the 128 x 36 array
//! (bit c of a row's hex value = column c); rows with nothing set are
//! skipped. For working out where a design's words landed.
//!
//!     cargo run --release --example m4k_content -- <top.rbf> <x> <y>
use ddb::asmdb::arch;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [rbf, x, y] = &a[..] else { anyhow::bail!("usage: m4k_content <rbf> <x> <y>") };
    let db = chipsmith_cyclone2::bitstream::phys::Db::load(&cram::db::locate()?)?;
    let rbf = chipsmith_cyclone2::bitstream::rbf::Rbf::read(std::path::Path::new(rbf))?;
    let f = db.m4k.feature(arch::RAM_CONTENT_YGR_MEAB, -1)?;
    let (x, y): (u32, u32) = (x.parse()?, y.parse()?);
    let base = db.m4k.base(x, y)?;
    let mut rows = vec![0u64; chipsmith_cyclone2::bitstream::m4k::ROWS];
    for (i, &off) in f.paths[0][0].iter().enumerate() {
        let flat = (base as i64 + off as i64) as u32;
        if rbf.get(chipsmith_cyclone2::bitstream::addr::frame_bit(flat)) {
            rows[i % chipsmith_cyclone2::bitstream::m4k::ROWS] |= 1 << (i / chipsmith_cyclone2::bitstream::m4k::ROWS);
        }
    }
    // Which LAB line each input mux and control input selects.
    let mut muxes: Vec<(String, i32, i32)> = cram::chipdb::m4k_input_pins();
    for k in 0..8 {
        muxes.push((format!("CTL{k}"), arch::RAM_PIA_TO_INPUT_YGR, k));
    }
    for (pin, at, grp) in muxes {
        let f = db.m4k.feature(at, grp)?;
        for s in 0..cram::chipdb::M4K_LINES {
            let Ok(bits) = f.source_bits(0, s) else { continue };
            if bits.iter().all(|&b| rbf.get(chipsmith_cyclone2::bitstream::addr::frame_bit((base as i64 + b as i64) as u32))) {
                println!("{pin} <- line {s}");
            }
        }
    }
    for (r, v) in rows.iter().enumerate() {
        if *v != 0 {
            println!("row {r:3}: {v:09x}");
        }
    }
    Ok(())
}
