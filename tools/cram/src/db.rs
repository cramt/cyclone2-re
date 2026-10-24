//! The device database this repo ships (`db/EP2C35F672C6/`), so that
//! packing, the base image and nextpnr's chip database need no Quartus
//! files at use time. `cram db` derives it from Quartus' devinfo once; every
//! use-time command reads only it.
//!
//! Two files, both xz:
//!
//! - `device.bin.xz`: `MAGIC`, `VERSION` (u32, little-endian), then the
//!   postcard encoding (<https://postcard.jamesmunns.com/wire-format>) of
//!   `Stored`: the routing graph, every routing element's CRAM bits, the
//!   asmdb's block tables and the package's user pins. The schema is the
//!   Rust types it names; the format and its reader are chipsmith's
//!   (`chipsmith_cyclone2::bitstream::db`), which reads this database.
//! - `chipdb.txt.xz`: `cram chipdb` output, the input of
//!   `nextpnr/cyclone2/gen/arch_gen.py` (nextpnr builds with Python only).
//!   `cram db` exports it from the `device.bin.xz` it just wrote, read back,
//!   so the two can't drift apart.

use anyhow::{bail, ensure, Result};
use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use lzma_rust2::{XzOptions, XzWriterMt};

use chipsmith_cyclone2::bitstream::db::{DEVICE, DEVICE_FILE};
use chipsmith_cyclone2::bitstream::io::{DualPurpose, Ioe, Pin, BLOCK_CLK_N, BLOCK_CLK_P, BLOCK_COL, BLOCK_ROW};
use chipsmith_cyclone2::bitstream::phys::Db;

pub const CHIPDB_FILE: &str = "chipdb.txt.xz";

/// Straight from Quartus' device files (`cram db` and the RE commands).
pub fn from_quartus(devinfo: &Path) -> Result<Db> {
    let asmdb = ddb::asmdb::parse(&ddb::container::Ddb::open(&devinfo.join("ddb_magellan4_asmdb.ddb"))?.body)?;
    let pins = pins_from_quartus(devinfo)?;
    Db::new(ddb::device::load(devinfo, "magellan4")?, asmdb, &pins)
}

/// Every bonded user pin of the F672 package and its IOE, from Quartus'
/// pkg and pad databases. Sorted by name, so the database `cram db` writes
/// doesn't depend on hash order.
pub fn pins_from_quartus(devinfo: &Path) -> Result<Vec<Pin>> {
    let (pkg, pad) = ddb::pins::load(devinfo, "magellan4", "fbga672")?;
    // Path order is pad-id order within each pad type; dedicated clock
    // inputs pair up in pad order, the p pad in block 60 and n in 61.
    // Observed: every output-data mux of the I/O fuzz designs is on the
    // path this predicts.
    let mut pads: Vec<_> = pad.pads.iter().filter(|p| p.loc.is_some()).collect();
    pads.sort_by_key(|p| p.id);
    let mut ioe_of = HashMap::new();
    let (mut row, mut col, mut clk) = (0, 0, 0);
    for p in &pads {
        let ioe = match p.io_type.as_deref() {
            Some("Row I/O") => (BLOCK_ROW, &mut row),
            Some("Column I/O") => (BLOCK_COL, &mut col),
            Some("Dedicated Clock") => {
                let n = p.alt_name.as_deref().is_some_and(|a| a.ends_with('A'));
                let path = clk / 2;
                clk += 1;
                ioe_of.insert(p.id, Ioe { block: if n { BLOCK_CLK_N } else { BLOCK_CLK_P }, path });
                continue;
            }
            _ => continue,
        };
        ioe_of.insert(p.id, Ioe { block: ioe.0, path: *ioe.1 });
        *ioe.1 += 1;
    }
    let mut pins = Vec::new();
    for m in ddb::pins::join(&pkg, &pad)? {
        let [p] = m.pads.as_slice() else { continue };
        let Some(&ioe) = ioe_of.get(&p.id) else { continue };
        let f = p.function.as_deref().unwrap_or("");
        let dual_purpose = if f.contains("nCEO") {
            Some(DualPurpose::Nceo)
        } else if f.contains("ASDO") || f.contains("nCSO") {
            Some(DualPurpose::SerialFlash)
        } else {
            None
        };
        pins.push(Pin { name: m.pin.name.clone(), ioe, xyz: p.loc.unwrap(), bank: p.bank, dual_purpose });
    }
    pins.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(pins)
}

/// Write `<root>/EP2C35F672C6/` from Quartus' device files, then read it
/// back: the reloaded database must encode to the same bytes, and the chip
/// database text comes from it rather than from Quartus' files.
pub fn build(devinfo: &Path, root: &Path) -> Result<()> {
    let dir = root.join(DEVICE);
    std::fs::create_dir_all(&dir)?;
    let encoded = from_quartus(devinfo)?.encode()?;
    write_xz(&dir.join(DEVICE_FILE), &encoded)?;
    let db = Db::load(&dir)?;
    ensure!(db.encode()? == encoded, "the device database does not read back as written");
    let mut text = Vec::new();
    crate::chipdb::export(&db, &mut text)?;
    write_xz(&dir.join(CHIPDB_FILE), &text)
}

/// The output must be a function of the input alone (it is committed):
/// fixed-size blocks, each compressed independently, come out the same
/// whatever the thread count. Preset 6's 8 MiB dictionary, so the 124 MB
/// chip database text compresses in parallel.
fn write_xz(path: &Path, data: &[u8]) -> Result<()> {
    let mut opts = XzOptions::with_preset(6);
    opts.set_block_size(NonZeroU64::new(BLOCK_SIZE));
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
    let tmp = path.with_extension("xz.tmp");
    let mut w = XzWriterMt::new(BufWriter::new(std::fs::File::create(&tmp)?), opts, threads)?;
    w.write_all(data)?;
    w.finish()?.flush()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

const BLOCK_SIZE: u64 = 16 << 20;

/// Where use-time commands find the database: `$CRAM_DB/EP2C35F672C6` if
/// set; else the `db/` of the checkout this binary was built in, found by
/// walking up from the executable (`tools/cram/target/<profile>/cram`) and,
/// failing that, at the path cargo built it from.
pub fn locate() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os("CRAM_DB") {
        return Ok(PathBuf::from(root).join(DEVICE));
    }
    let has_db = |root: &Path| root.join("db").join(DEVICE).join(DEVICE_FILE).is_file();
    let exe = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok());
    let built_in = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    match exe.iter().flat_map(|e| e.ancestors()).find(|a| has_db(a)) {
        Some(root) => Ok(root.join("db").join(DEVICE)),
        None if has_db(&built_in) => Ok(built_in.join("db").join(DEVICE)),
        None => bail!("no device database (db/{DEVICE}/{DEVICE_FILE}) above the cram executable or in its source tree; set CRAM_DB to the db/ directory"),
    }
}
