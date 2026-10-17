use anyhow::{bail, Result};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use cram::{addr, design::Design, le, rbf::{self, FrameBit, Rbf}};
use ddb::device::Device;

fn usage() -> ! {
    eprintln!(
        "usage (CRAM_DEVINFO overrides the Quartus devinfo/cycloneii dir):
  cram check <empty.rbf> <design dir>...   predicted routing bits vs RBF diff
  cram addr <flat>...                      flat CRAM address -> frame:bit
  cram routing <design dir>                routing bits an rcf sets, as fN:B
  cram lab-check                           asmdb LAB tables vs the fuzz-fit LUT
                                           geometry and learned LE input mux
  cram assemble <empty.rbf> <design dir> <out.rbf> [--io <training dir>...]
      empty RBF + modelled bits, check words recomputed. --io adds the
      I/O bits every training design shares (same pin set), learned as is.
  cram compare <ours.rbf> <quartus.rbf>    categorised bit diff
  cram verify <rbf>...                     do all check words match?"
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
        add("le/io mux predicted", mux_ones.len());
        add("le/io mux hit", mux_ones.intersection(&obs).count());
        add("io outputs unmodelled", des.unmodelled_io_outs + des.io_outs.iter().filter(|o| le::io_out_mux(o.x, o.z, o.line).is_none()).count());
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

/// I/O-region bits (frames 0-23, top/bottom I/O rows) that differ from
/// the empty design identically in every training design.
fn shared_io_bits(base: &Rbf, dirs: &[String]) -> Result<Vec<(FrameBit, bool)>> {
    let mut common: Option<HashSet<(FrameBit, bool)>> = None;
    for d in dirs {
        let rbf = Rbf::read(&Path::new(d).join("build/output_files/top.rbf"))?;
        let s: HashSet<(FrameBit, bool)> =
            base.diff(&rbf).into_iter().filter(|b| is_io_region(*b)).map(|b| (b, rbf.get(b))).collect();
        common = Some(match common {
            None => s,
            Some(c) => c.intersection(&s).copied().collect(),
        });
    }
    let mut v: Vec<_> = common.unwrap_or_default().into_iter().collect();
    v.sort();
    Ok(v)
}

fn is_io_region(b: FrameBit) -> bool {
    if (b.frame as usize) < rbf::FIRST_CRC_FRAME {
        return true;
    }
    addr::flat(b).map_or(true, |f| {
        let row = f / addr::COLUMNS;
        row < le::lab_row(35) || row >= le::lab_row(1) + le::LAB_ROWS
    })
}

fn assemble(empty: &Path, dir: &Path, out: &Path, io_dirs: &[String]) -> Result<()> {
    let dev = Device::load(&devinfo(), "magellan4")?;
    let mut rbf = Rbf::read(empty)?;
    let des = Design::from_fuzz_dir(&dev, dir)?;
    let (lut, mux) = des.le_ones();
    for b in des.routing.iter().chain(&lut).chain(&mux) {
        rbf.set(*b, true);
    }
    let io = shared_io_bits(&Rbf::read(empty)?, io_dirs)?;
    for &(b, v) in &io {
        rbf.set(b, v);
    }
    rbf.fix_crcs();
    // Frames 0-23's check words don't follow the CRC of the other frames
    // and we can't compute them yet. If every training design has the same
    // bytes there (true for a fixed pin set), take them verbatim.
    if let Some(first) = io_dirs.first() {
        let t = Rbf::read(&Path::new(first).join("build/output_files/top.rbf"))?;
        let mut same = true;
        for d in &io_dirs[1..] {
            let o = Rbf::read(&Path::new(d).join("build/output_files/top.rbf"))?;
            same &= (0..rbf::FIRST_CRC_FRAME).all(|f| o.frame_bytes(f) == t.frame_bytes(f));
        }
        if same {
            for f in 0..rbf::FIRST_CRC_FRAME {
                rbf.set_frame_bytes(f, t.frame_bytes(f));
            }
            eprintln!("frames 0-23 identical in all {} training designs: copied with their check words", io_dirs.len());
        }
    }
    std::fs::write(out, rbf.bytes())?;
    eprintln!(
        "assembled {}: {} routing, {} LUT, {} LE/IO-mux bits, {} learned I/O bits",
        out.display(), des.routing.len(), lut.len(), mux.len(), io.len()
    );
    Ok(())
}

fn compare(ours: &Rbf, theirs: &Rbf) {
    let total = ours.bytes().iter().zip(theirs.bytes()).map(|(a, b)| (a ^ b).count_ones()).sum::<u32>();
    let data = ours.diff(theirs);
    let mut cats = BTreeMap::<&str, usize>::new();
    for b in &data {
        let k = if (b.frame as usize) < rbf::FIRST_CRC_FRAME {
            "frames 0-23"
        } else if is_io_region(*b) {
            "top/bottom I/O rows"
        } else {
            "core"
        };
        *cats.entry(k).or_default() += 1;
    }
    let bad_crc = (rbf::FIRST_CRC_FRAME..rbf::FRAMES).filter(|&f| !ours.frame_crc_ok(f)).count()
        + (0..rbf::REP_FRAMES).filter(|&f| !ours.rep_frame_ok(f)).count();
    println!(
        "{} differing bits in the whole file; {} in frame data {:?}; header/check-word/tail bits {}; \
         frames with a bad check word in ours: {bad_crc}",
        total, data.len(), cats, total as usize - data.len()
    );
    if std::env::var_os("CRAM_VERBOSE").is_some() {
        for b in &data {
            println!("  {b} ours={} quartus={}", u8::from(ours.get(*b)), u8::from(theirs.get(*b)));
        }
    }
}

fn asmdb() -> Result<ddb::asmdb::Asmdb> {
    let p = devinfo().join("ddb_magellan4_asmdb.ddb");
    ddb::asmdb::parse(&ddb::container::Ddb::open(&p)?.body)
}

/// The asmdb's LUT bit positions must be the fuzz-fit `le::lut_bit` model
/// for every LE of every LAB, and its LE input mux must reproduce every
/// entry of the learned table.
fn lab_check() -> Result<()> {
    use ddb::asmdb::arch;
    let lab = cram::lab::LabDb::new(&asmdb()?)?;
    let lut = lab.feature(arch::LE_LUT_FUNCTION, -1)?;
    let mut n = 0;
    for x in 1..=64 {
        for y in 1..=35 {
            if le::lut_frame(x).is_none() || (x <= 6 && (19..=22).contains(&y)) {
                continue;
            }
            for l in 0..16u32 {
                for i in 0..16 {
                    // asmdb mux setting i is truth-table entry 15 - i.
                    let (frame, row) = le::lut_bit(x, y, 2 * l, 15 - i).unwrap();
                    let fb = addr::frame_bit(lab.flat(x, y, lut.paths[l as usize][0][i as usize])?);
                    anyhow::ensure!(fb == addr::frame_bit(row * addr::COLUMNS + addr::LAST_FRAME - frame),
                        "LUT X{x} Y{y} LE{l} bit {i}: asmdb {fb}, model f{frame} row {row}");
                    n += 1;
                }
            }
        }
    }
    println!("{} LABs; {n} LUT bits agree with the fuzz-fit model", lab.labs());
    let (mut ok, mut tot) = (0, 0);
    for &(line, port, bits) in cram::le_mux_table::LE_INPUT_MUX {
        let f = lab.feature(arch::LE_LUT_INPUT, port as i32)?;
        for l in 0..16u32 {
            let got: std::collections::BTreeSet<_> = f
                .source_bits(l as usize, line as usize)?
                .into_iter()
                .map(|o| addr::frame_bit(lab.flat(30, 10, o).unwrap()))
                .collect();
            let want: std::collections::BTreeSet<_> = bits
                .iter()
                .map(|&(df, r)| {
                    let (frame, row) = le::le_bit(30, 10, 2 * l, df as i32, r as u32).unwrap();
                    addr::frame_bit(row * addr::COLUMNS + addr::LAST_FRAME - frame)
                })
                .collect();
            tot += 1;
            ok += usize::from(got == want);
        }
    }
    println!("learned LE input mux: {ok}/{tot} (line, port, LE) entries reproduced by the asmdb");
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { usage() };
    match (cmd.as_str(), &args[1..]) {
        ("check", [empty, dirs @ ..]) if !dirs.is_empty() => check(Path::new(empty), dirs)?,
        ("assemble", [empty, dir, out, rest @ ..]) => {
            let io_dirs = match rest {
                [] => &[][..],
                [flag, dirs @ ..] if flag == "--io" => dirs,
                _ => usage(),
            };
            assemble(Path::new(empty), Path::new(dir), Path::new(out), io_dirs)?
        }
        ("compare", [ours, theirs]) => compare(&Rbf::read(Path::new(ours))?, &Rbf::read(Path::new(theirs))?),
        ("verify", files) if !files.is_empty() => {
            for f in files {
                let r = Rbf::read(Path::new(f))?;
                let mut fixed = r.clone();
                fixed.fix_crcs();
                println!("{f}: {}", if fixed == r { "ok" } else { "check words differ" });
            }
        }
        ("routing", [dir]) => {
            // Exploration aid: every routing bit an rcf sets, plus what didn't
            // resolve, for scripts that model the rest.
            let dev = Device::load(&devinfo(), "magellan4")?;
            let sigs = cram::rcf::read(&Path::new(dir).join("build/top.rcf"))?;
            let (bits, skipped) = cram::route::bits(&dev, &sigs)?;
            for b in bits {
                println!(
                    "{} enc={} high={} strange={} cff={}",
                    addr::frame_bit(b.flat_address), b.use_encoded_setting, b.is_encoded_bit_high,
                    b.is_strangely_encoded, b.is_cff_bit
                );
            }
            for r in skipped {
                eprintln!("skipped {r:?}");
            }
        }
        ("lab-check", []) => lab_check()?,
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
