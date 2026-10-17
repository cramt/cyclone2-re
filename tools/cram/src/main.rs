use anyhow::{bail, Result};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use cram::{addr, design::Design, le, rbf::{self, FrameBit, Rbf}};
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

/// Residual bit -> coarse category, by CRAM geometry alone.
fn categorise(b: FrameBit, used: &HashSet<(u32, u32)>, used_le_rows: &HashSet<u32>, hist: &mut BTreeMap<i64, usize>) -> &'static str {
    if (b.frame as usize) < rbf::FIRST_CRC_FRAME {
        return "frames 0-23";
    }
    let Some(flat) = addr::flat(b) else { return "unaddressable" };
    let row = flat / addr::COLUMNS;
    let frame = b.frame as u32;
    if row < le::lab_row(35) || row >= le::lab_row(1) + le::LAB_ROWS {
        return "top/bottom I/O rows";
    }
    let y = 35 - (row - le::lab_row(35)) / le::LAB_ROWS;
    let off = row - le::lab_row(y);
    // Nearest LAB column by LUT anchor.
    let Some((x, d)) = (1..=64)
        .filter_map(|x| le::lut_frame(x).map(|f| (x, frame as i64 - f as i64)))
        .min_by_key(|(_, d)| d.abs())
    else {
        unreachable!()
    };
    if !(LAB_SPAN.0..LAB_SPAN.1).contains(&d) {
        return "non-LAB column";
    }
    if !used.contains(&(x, y)) {
        return "LAB, unused";
    }
    *hist.entry(d).or_default() += 1;
    if (32..38).contains(&off) {
        "used LAB, rows 32-37"
    } else if used_le_rows.contains(&row) {
        "used LE rows, not LUT"
    } else {
        "used LAB, other LE rows"
    }
}

/// Frames of a LAB column relative to its first LUT frame. Guess, refined
/// from the residual histogram.
const LAB_SPAN: (i64, i64) = (-17, 18);

fn check(empty: &Path, dirs: &[String]) -> Result<()> {
    let dev = Device::load(&devinfo(), "magellan4")?;
    let base = Rbf::read(empty)?;
    let mut tot = BTreeMap::<String, usize>::new();
    let mut hist = BTreeMap::<i64, usize>::new();
    for d in dirs {
        let dir = Path::new(d);
        let des = Design::from_fuzz_dir(&dev, dir)?;
        let rbf = Rbf::read(&dir.join("build/output_files/top.rbf"))?;
        let obs: HashSet<FrameBit> = base.diff(&rbf).into_iter().collect();
        let routing: HashSet<FrameBit> = des.routing.iter().copied().collect();
        let lut_all = des.lut_bits();
        let (lut_ones, mux_ones) = des.le_ones();
        let ones: HashSet<FrameBit> = routing.iter().chain(&lut_ones).chain(&mux_ones).copied().collect();
        let mut c = BTreeMap::<String, usize>::new();
        let mut add = |k: &str, v: usize| *c.entry(k.to_string()).or_default() += v;
        add("observed", obs.len());
        add("luts with unknown mask", des.unknown_luts);
        add("routing predicted", routing.len());
        add("routing hit", routing.intersection(&obs).count());
        add("routing polarity err", routing.iter().filter(|b| obs.contains(b) && !rbf.get(**b)).count());
        add("lut predicted", lut_ones.len());
        add("lut hit", lut_ones.intersection(&obs).count());
        // LUT entries predicted to stay 0 but observed flipped.
        add("lut wrong", lut_all.difference(&lut_ones).filter(|b| obs.contains(b)).count());
        add("le mux predicted", mux_ones.len());
        add("le mux hit", mux_ones.intersection(&obs).count());
        let used: HashSet<(u32, u32)> = des.les.iter().map(|l| (l.loc.x, l.loc.y)).collect();
        let used_le_rows: HashSet<u32> = des
            .les
            .iter()
            .flat_map(|l| (0..16).map(move |p| le::lut_bit(l.loc.x, l.loc.y, l.loc.n, p).unwrap().1))
            .collect();
        for b in obs.iter().filter(|b| !ones.contains(b) && !lut_all.contains(b)) {
            add(&format!("residual: {}", categorise(*b, &used, &used_le_rows, &mut hist)), 1);
        }
        println!("{d}: {c:?}");
        for (k, v) in c {
            *tot.entry(k).or_default() += v;
        }
    }
    println!("total over {} designs: {tot:#?}", dirs.len());
    if std::env::var_os("CRAM_VERBOSE").is_some() {
        println!("residual frame - lut_frame histogram (used LABs): {hist:?}");
    }
    Ok(())
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
