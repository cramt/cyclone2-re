use anyhow::{bail, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use cram::{addr, rbf::{FrameBit, Rbf}, rcf, route};
use ddb::device::Device;

fn usage() -> ! {
    eprintln!(
        "usage (CRAM_DEVINFO overrides the Quartus devinfo/cycloneii dir):
  cram check <empty.rbf> <design dir>...   predicted routing bits vs RBF diff
  cram addr <flat>...                      flat CRAM address -> frame:bit"
    );
    std::process::exit(2)
}

fn devinfo() -> PathBuf {
    std::env::var_os("CRAM_DEVINFO").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap()).join("altera/13.0sp1/quartus/common/devinfo/cycloneii")
    })
}

/// Which tiles each flat-address column / row holds, read off the asm bits
/// of every routing element: lets residual bits be named by tile.
struct TileIndex {
    col_x: HashMap<u32, HashSet<i16>>,
    row_y: HashMap<u32, HashSet<i16>>,
}

impl TileIndex {
    fn new(dev: &Device) -> Self {
        let (mut col_x, mut row_y) = (HashMap::<u32, HashSet<i16>>::new(), HashMap::<u32, HashSet<i16>>::new());
        for (i, e) in dev.routing.elements.iter().enumerate() {
            for b in &dev.asm[i].bits {
                col_x.entry(b.flat_address % addr::COLUMNS).or_default().insert(e.x);
                row_y.entry(b.flat_address / addr::COLUMNS).or_default().insert(e.y);
            }
        }
        TileIndex { col_x, row_y }
    }
}

fn design_bits(dev: &Device, dir: &Path) -> Result<(HashSet<FrameBit>, usize, usize)> {
    let sigs = rcf::read(&dir.join("build/top.rcf"))?;
    let (bits, skipped) = route::bits(dev, &sigs)?;
    let n = bits.len();
    Ok((bits.iter().map(|b| addr::frame_bit(b.flat_address)).collect(), n, skipped.len()))
}

fn check(empty: &Path, dirs: &[String]) -> Result<()> {
    let dev = Device::load(&devinfo(), "magellan4")?;
    let base = Rbf::read(empty)?;
    let tiles = TileIndex::new(&dev);
    let mut tot = BTreeMap::<&str, usize>::new();
    for d in dirs {
        let dir = Path::new(d);
        let (pred, nbits, skipped) = design_bits(&dev, dir)?;
        let rbf = Rbf::read(&dir.join("build/output_files/top.rbf"))?;
        let obs: HashSet<FrameBit> = base.diff(&rbf).into_iter().collect();
        let hit = pred.iter().filter(|b| obs.contains(b)).count();
        // Routing bits are active high: every hit should be 0 -> 1.
        let wrong_pol = pred.iter().filter(|b| obs.contains(b) && !rbf.get(**b)).count();
        let mut cats = BTreeMap::<&str, usize>::new();
        for b in obs.difference(&pred) {
            *cats.entry(categorise(&tiles, *b)).or_default() += 1;
        }
        println!(
            "{d}: predicted {} ({nbits} asm bits, {skipped} rcf resources not routing), observed {}, \
             predicted&observed {hit}, predicted-only {}, polarity errors {wrong_pol}, unexplained {:?}",
            pred.len(), obs.len(), pred.len() - hit, cats
        );
        *tot.entry("predicted").or_default() += pred.len();
        *tot.entry("hit").or_default() += hit;
        *tot.entry("observed").or_default() += obs.len();
        for (k, v) in cats {
            *tot.entry(k).or_default() += v;
        }
    }
    println!("total: {tot:?}");
    Ok(())
}

/// Name the tile a bit sits in, from which routing elements' asm bits share
/// its column and row. Inverse of `addr::frame_bit` for frames >= 24.
fn categorise(t: &TileIndex, b: FrameBit) -> &'static str {
    if (b.frame as usize) < cram::rbf::FIRST_CRC_FRAME {
        return "frames0-23";
    }
    let Some(flat) = addr::flat(b) else { return "unaddressable" };
    let col = flat % addr::COLUMNS;
    let row = flat / addr::COLUMNS;
    let xs = t.col_x.get(&col);
    let ys = t.row_y.get(&row);
    let edge_x = xs.is_some_and(|s| s.iter().any(|&x| x == 0 || x == 65));
    let edge_y = ys.is_some_and(|s| s.iter().any(|&y| y == 0 || y == 36));
    match (xs, ys) {
        (None, _) | (_, None) => "no-routing-tile",
        _ if edge_x || edge_y => "io-tile",
        _ => "core-tile",
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { usage() };
    match (cmd.as_str(), &args[1..]) {
        ("check", [empty, dirs @ ..]) if !dirs.is_empty() => check(Path::new(empty), dirs)?,
        ("addr", flats) => {
            for f in flats {
                println!("{f} {}", addr::frame_bit(f.parse()?));
            }
        }
        _ => {
            if !cmd.is_empty() {
                bail!("unknown command {cmd:?}");
            }
            usage()
        }
    }
    Ok(())
}
