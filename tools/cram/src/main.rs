use anyhow::{bail, Result};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use cram::{addr, le, phys::{Bits, Db, PhysDesign}, quartus, rbf::{self, FrameBit, Rbf, RepBit}};
use ddb::device::Device;

fn usage() -> ! {
    eprintln!(
        "usage (CRAM_DEVINFO overrides the Quartus devinfo/cycloneii dir):
  cram check <empty.rbf> <design dir>...   modelled bits vs the design's RBF, residuals
                                           (needs build/top.rcf and the post-fit
                                           netlist from `just netlist`)
  cram addr <flat>...                      flat CRAM address -> frame:bit
  cram routing <design dir>                routing bits an rcf sets, as fN:B
  cram lab-check                           asmdb LAB tables vs the fuzz-fit LUT geometry
  cram assemble <empty.rbf> <design dir> <out.rbf>
      empty RBF + modelled bits (routing, logic, every pin's IOE), check
      words recomputed.
  cram compare <ours.rbf> <quartus.rbf>    categorised bit diff
  cram roundtrip <empty.rbf> <design dir>... assemble each and compare with its RBF
  cram verify <rbf>...                     do all check words match?
  cram pack <empty.rbf> <routed.json> <out.rbf>
      nextpnr routed design (himbaechel --write) -> RBF
  cram quartus-export <routed.json> <dir>  the same placement and routing as Quartus input
                                           (top.vqm, assignments.qsf, routing.rcf)
  cram blank <out.rbf>                     base image built from the device DB alone; any
                                           <empty.rbf> argument can be `-` for it
  cram chipdb <out.txt>                    nextpnr chip database description (for
                                           nextpnr/cyclone2/gen_chipdb.py)"
    );
    std::process::exit(2)
}

fn devinfo() -> PathBuf {
    std::env::var_os("CRAM_DEVINFO").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap()).join("altera/13.0sp1/quartus/common/devinfo/cycloneii")
    })
}

/// Residual bit -> coarse category, by CRAM geometry alone.
fn categorise(b: FrameBit, used: &HashSet<(u32, u32)>) -> &'static str {
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
    // A LAB block spans frames lut_frame - 22 ..= lut_frame + 12 (asmdb).
    let x = (1..=64).find(|&x| le::lut_frame(x).is_some_and(|f| (f as i64 - 22..=f as i64 + 12).contains(&(frame as i64))));
    match x {
        None => "non-LAB column",
        Some(x) if used.contains(&(x, y)) => "used LAB",
        Some(_) => "unused LAB",
    }
}

fn load_db() -> Result<Db> {
    let db = asmdb()?;
    Ok(Db {
        dev: Device::load(&devinfo(), "magellan4")?,
        lab: cram::lab::LabDb::new(&db)?,
        io: cram::io::IoDb::new(&devinfo(), &db)?,
        asmdb: db,
    })
}

fn check(empty: &Path, dirs: &[String]) -> Result<()> {
    let m = load_db()?;
    let base = base(&m, empty)?;
    let mut tot = BTreeMap::<String, usize>::new();
    for d in dirs {
        let dir = Path::new(d);
        let q = quartus::load(&m.dev, dir)?;
        let des = q.design.bits(&m)?;
        let rbf = Rbf::read(&dir.join("build/output_files/top.rbf"))?;
        // Raw frames 0-23 are the replicated frames, compared separately.
        let obs: HashSet<FrameBit> =
            base.diff(&rbf).into_iter().filter(|b| b.frame as usize >= rbf::FIRST_CRC_FRAME).collect();
        let rep_obs: HashSet<RepBit> = base.rep_diff(&rbf).into_iter().collect();
        let mut c = BTreeMap::<String, usize>::new();
        let mut add = |k: String, v: usize| *c.entry(k).or_default() += v;
        add("observed flips".into(), obs.len());
        add("conflicting predictions".into(), des.conflicts().len());
        add("pins".into(), q.design.pins.len());
        add("replicated-frame flips".into(), rep_obs.len());
        let mut rep_predicted = HashSet::new();
        for &(b, v) in &des.rep {
            rep_predicted.insert(b);
            add("Io (replicated frames) predicted".into(), 1);
            add(format!("Io (replicated frames) {}", if rbf.rep_get(b) == v { "right" } else { "WRONG" }), 1);
        }
        add("residual: replicated frames".into(), rep_obs.difference(&rep_predicted).count());
        for u in &q.unmodelled {
            eprintln!("{d}: unmodelled: {u}");
        }
        add("logic features unmodelled".into(), q.unmodelled.len());
        let mut predicted = HashSet::new();
        for &(b, v, src) in &des.core {
            predicted.insert(b);
            let name = format!("{src:?}");
            add(format!("{name} predicted"), 1);
            if rbf.get(b) == v {
                add(format!("{name} right"), 1);
            } else {
                add(format!("{name} WRONG"), 1);
                if std::env::var_os("CRAM_VERBOSE").is_some() {
                    eprintln!("{d}: {name} wrong at {b}: want {}", u8::from(rbf.get(b)));
                }
            }
        }
        let used: HashSet<(u32, u32)> = q.design.logic.les.iter().map(|l| (l.x, l.y)).collect();
        for b in obs.iter().filter(|b| !predicted.contains(b)) {
            add(format!("residual: {}", categorise(*b, &used)), 1);
            if std::env::var_os("CRAM_VERBOSE").is_some() {
                eprintln!("{d}: residual {b} {}", categorise(*b, &used));
            }
        }
        println!("{d}: {c:?}");
        for (k, v) in c {
            *tot.entry(k).or_default() += v;
        }
    }
    println!("total over {} designs: {tot:#?}", dirs.len());
    Ok(())
}

fn is_io_region(b: FrameBit) -> bool {
    if (b.frame as usize) < rbf::FIRST_CRC_FRAME {
        return true;
    }
    // Left and right I/O columns: the frames outside the outermost LAB
    // blocks (a LAB block spans lut_frame - 22 ..= lut_frame + 12, asmdb).
    let frame = b.frame as u32;
    if frame < le::lut_frame(1).unwrap() - 22 || frame > le::lut_frame(64).unwrap() + 12 {
        return true;
    }
    addr::flat(b).map_or(true, |f| {
        let row = f / addr::COLUMNS;
        row < le::lab_row(35) || row >= le::lab_row(1) + le::LAB_ROWS
    })
}

/// The base image: an RBF file, or `-` for the one `blank` builds.
fn base(m: &Db, arg: &Path) -> Result<Rbf> {
    if arg == Path::new("-") {
        cram::blank::image(&m.asmdb, &m.io)
    } else {
        Rbf::read(arg)
    }
}

/// Base image + the physical design's bits, check words recomputed.
fn pack(m: &Db, empty: &Path, des: &PhysDesign) -> Result<(Rbf, Bits)> {
    let mut rbf = base(m, empty)?;
    let bits = des.bits(m)?;
    anyhow::ensure!(bits.conflicts().is_empty(), "model sets {} bits both ways", bits.conflicts().len());
    for &(b, v, _) in &bits.core {
        rbf.set(b, v);
    }
    for &(b, v) in &bits.rep {
        rbf.rep_set(b, v);
    }
    rbf.fix_crcs();
    Ok((rbf, bits))
}

fn report(out: &Path, des: &PhysDesign, bits: &Bits) {
    let mut by = BTreeMap::<String, usize>::new();
    for &(_, _, s) in &bits.core {
        *by.entry(format!("{s:?}")).or_default() += 1;
    }
    eprintln!(
        "assembled {}: model bits {by:?} + {} replicated-frame I/O bits, {} LEs, {} pins, {} nets",
        out.display(), bits.rep.len(), des.logic.les.len(), des.pins.len(), des.nets.len()
    );
}

fn assemble(empty: &Path, dir: &Path, out: &Path) -> Result<()> {
    let m = load_db()?;
    let q = quartus::load(&m.dev, dir)?;
    for u in &q.unmodelled {
        eprintln!("unmodelled: {u}");
    }
    let (rbf, bits) = pack(&m, empty, &q.design)?;
    std::fs::write(out, rbf.bytes())?;
    report(out, &q.design, &bits);
    Ok(())
}

/// Assemble each design dir and compare with Quartus's RBF in it.
fn roundtrip(empty: &Path, dirs: &[String]) -> Result<()> {
    let m = load_db()?;
    let mut same = 0;
    for d in dirs {
        let theirs = Rbf::read(&Path::new(d).join("build/output_files/top.rbf"))?;
        match quartus::load(&m.dev, Path::new(d)).and_then(|q| Ok((pack(&m, empty, &q.design)?.0, q.design))) {
            Ok((ours, des)) => {
                if ours == theirs {
                    same += 1;
                    println!("{d}: byte-identical ({} pins)", des.pins.len());
                } else {
                    print!("{d}: ");
                    compare(&ours, &theirs);
                }
            }
            Err(e) => println!("{d}: not assembled: {e:#}"),
        }
    }
    println!("{same} of {} byte-identical", dirs.len());
    Ok(())
}

fn compare(ours: &Rbf, theirs: &Rbf) {
    let total = ours.bytes().iter().zip(theirs.bytes()).map(|(a, b)| (a ^ b).count_ones()).sum::<u32>();
    let data: Vec<FrameBit> =
        ours.diff(theirs).into_iter().filter(|b| b.frame as usize >= rbf::FIRST_CRC_FRAME).collect();
    let rep = ours.rep_diff(theirs);
    let mut cats = BTreeMap::<&str, usize>::new();
    cats.insert("replicated frames", rep.len());
    for b in &data {
        let k = if is_io_region(*b) {
            "I/O rows/columns"
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
        total, data.len() + rep.len(), cats, total as usize - data.len() - 4 * rep.len()
    );
    if std::env::var_os("CRAM_VERBOSE").is_some() {
        for b in &rep {
            println!("  {b} ours={} quartus={}", u8::from(ours.rep_get(*b)), u8::from(theirs.rep_get(*b)));
        }
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
/// for every LE of every LAB. (It also reproduced all 608 entries of the
/// LE input-mux table learned from the routing fuzz designs, which it
/// replaced; README.)
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
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { usage() };
    match (cmd.as_str(), &args[1..]) {
        ("check", [empty, dirs @ ..]) if !dirs.is_empty() => check(Path::new(empty), dirs)?,
        ("assemble", [empty, dir, out]) => assemble(Path::new(empty), Path::new(dir), Path::new(out))?,
        ("roundtrip", [empty, dirs @ ..]) if !dirs.is_empty() => roundtrip(Path::new(empty), dirs)?,
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
            let (bits, skipped) = cram::route::bits(&dev, &sigs);
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
        ("pack", [empty, routed, out]) => {
            let m = load_db()?;
            let r = cram::nextpnr::read(&m.dev, Path::new(routed))?;
            let des = r.to_phys(&m.dev, &m.io)?;
            let (rbf, bits) = pack(&m, Path::new(empty), &des)?;
            std::fs::write(out, rbf.bytes())?;
            report(Path::new(out), &des, &bits);
        }
        ("quartus-export", [routed, dir]) => {
            let m = load_db()?;
            let r = cram::nextpnr::read(&m.dev, Path::new(routed))?;
            let q = r.to_quartus(&m.dev, &m.io)?;
            let dir = Path::new(dir);
            std::fs::create_dir_all(dir)?;
            std::fs::write(dir.join("top.vqm"), q.vqm)?;
            std::fs::write(dir.join("assignments.qsf"), q.qsf)?;
            std::fs::write(dir.join("routing.rcf"), q.rcf)?;
        }
        // Debugging aid: the physical netlist either frontend builds, one
        // item per line, for diffing.
        ("dump", [kind, src]) => {
            let m = load_db()?;
            let des = match kind.as_str() {
                "quartus" => quartus::load(&m.dev, Path::new(src))?.design,
                "nextpnr" => cram::nextpnr::read(&m.dev, Path::new(src))?.to_phys(&m.dev, &m.io)?,
                _ => usage(),
            };
            let mut lines = Vec::new();
            for l in &des.logic.les {
                lines.push(format!("{l:?}"));
            }
            for (xy, l) in &des.logic.labs {
                lines.push(format!("LAB {xy:?} {l:?}"));
            }
            for c in &des.logic.clkctrls {
                lines.push(format!("{c:?}"));
            }
            for p in &des.pins {
                let mut p = p.clone();
                p.port.clear();
                lines.push(format!("{p:?}"));
            }
            for n in &des.nets {
                let mut pips: Vec<String> = n.pips.iter().map(|p| format!("{}->{}", p.src().0, p.dst().0)).collect();
                pips.sort();
                lines.push(format!("net {}", pips.join(" ")));
            }
            lines.sort();
            for l in lines {
                println!("{l}");
            }
        }
        ("io-bits", [a, b]) => {
            // Debugging aid: differing bits of two RBFs that belong to an IOE.
            let m = load_db()?;
            let (a, b) = (Rbf::read(Path::new(a))?, Rbf::read(Path::new(b))?);
            for fb in a.diff(&b) {
                if let Some(d) = m.io.describe(cram::io::IoBit::Core(fb)) {
                    println!("{fb} {}->{} {d:?}", u8::from(a.get(fb)), u8::from(b.get(fb)));
                }
            }
            for rb in a.rep_diff(&b) {
                if let Some(d) = m.io.describe(cram::io::IoBit::Rep(rb)) {
                    println!("{rb} {}->{} {d:?}", u8::from(a.rep_get(rb)), u8::from(b.rep_get(rb)));
                }
            }
        }
        ("blank", [out]) => {
            let m = load_db()?;
            std::fs::write(out, cram::blank::image(&m.asmdb, &m.io)?.bytes())?;
        }
        ("chipdb", [out]) => {
            let m = load_db()?;
            let mut w = std::io::BufWriter::new(std::fs::File::create(out)?);
            cram::chipdb::export(&m, &mut w)?;
        }
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
