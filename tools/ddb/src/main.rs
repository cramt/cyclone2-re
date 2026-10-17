use ddb::{asm, container, export, pdb, pins, routing};

use anyhow::{bail, Result};
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "usage:
  ddb info <file.ddb>                 header + class table as JSON
  ddb unpack <file.ddb> <dir>         write decompressed body / footer
  ddb export <devinfo> <die> <out>    icname/routing/asm -> CSV + summary.json
                                      (die e.g. magellan4 = EP2C35)
  ddb routing-stats <routing.ddb>
  ddb routing-canon <routing.ddb>     canonical text, see verify/README.md
  ddb asm-canon <asm.ddb>
  ddb pins <devinfo> <die> <package>  package pin -> pad -> IOE x,y,z as CSV
                                      (e.g. magellan4 fbga672)
  ddb pins-trace <pad|pkg ddb>        value stream, see verify/README.md"
    );
    std::process::exit(2)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { usage() };
    match (cmd.as_str(), &args[1..]) {
        ("info", [f]) => {
            let d = container::Ddb::open(&PathBuf::from(f))?;
            let classes = pdb::parse_class_table(&d.footer)?;
            let j = serde_json::json!({ "header": d.header, "classes": classes });
            println!("{}", serde_json::to_string_pretty(&j)?);
        }
        ("unpack", [f, dir]) => {
            let d = container::Ddb::open(&PathBuf::from(f))?;
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir)?;
            let stem = PathBuf::from(f).file_stem().unwrap().to_string_lossy().into_owned();
            std::fs::write(dir.join(format!("{stem}.body.bin")), &d.body)?;
            std::fs::write(dir.join(format!("{stem}.footer.bin")), &d.footer)?;
        }
        ("routing-stats", [f]) => {
            let d = container::Ddb::open(&PathBuf::from(f))?;
            let t0 = std::time::Instant::now();
            let r = routing::parse(&d.body)?;
            let edges: usize = (0..r.elements.len()).map(|i| r.fanout(i).count()).sum();
            eprintln!("parsed in {:?}", t0.elapsed());
            println!("elements={} templates={} edges={}", r.elements.len(), r.templates.len(), edges);
        }
        ("routing-canon", [f]) => {
            // One line per element and per template, for diffing against a
            // PDB_ASCII_DUMP of the same file.
            use std::io::Write;
            let d = container::Ddb::open(&PathBuf::from(f))?;
            let r = routing::parse(&d.body)?;
            let mut o = std::io::BufWriter::new(std::io::stdout().lock());
            for e in &r.elements {
                writeln!(o, "E {} {} {:08X} {}", e.x, e.y, e.fanout_edge_base, e.template + 1)?;
            }
            for t in &r.templates {
                write!(o, "T {} {} {} {} {} {} {} {}", t.element_enum, t.num_edges, t.z, t.index,
                    t.first_purely_redundant_edge, t.length, t.metal_layer, t.direction)?;
                for v in &t.fanout_offsets { write!(o, " {v}")?; }
                write!(o, " |")?;
                for v in &t.fanout_to_positions { write!(o, " {v}")?; }
                writeln!(o)?;
            }
        }
        ("asm-canon", [f]) => {
            use std::io::Write;
            let d = container::Ddb::open(&PathBuf::from(f))?;
            let mut o = std::io::BufWriter::new(std::io::stdout().lock());
            for n in asm::parse(&d.body)? {
                write!(o, "N {} {} |", n.start_bit_index.len(), n.bits.len())?;
                for s in &n.start_bit_index { write!(o, " {s}")?; }
                write!(o, " |")?;
                for b in &n.bits {
                    write!(o, " {},{},{},{},{}", b.flat_address, b.use_encoded_setting, b.is_encoded_bit_high, b.is_strangely_encoded, b.is_cff_bit)?;
                }
                writeln!(o)?;
            }
        }
        ("pins", [dir, die, package]) => {
            use std::io::Write;
            let (pkg, pad) = pins::load(&PathBuf::from(dir), die, package)?;
            let mut o = std::io::BufWriter::new(std::io::stdout().lock());
            writeln!(o, "pin,pad,x,y,z,bank,io_type,pad_name,function,alt_name,pin_function")?;
            let q = |s: &Option<String>| match s.as_deref() {
                Some(s) if s.contains(',') => format!("\"{s}\""),
                s => s.unwrap_or("").to_string(),
            };
            for m in pins::join(&pkg, &pad)? {
                let p = m.pin;
                if m.pads.is_empty() {
                    writeln!(o, "{},,,,,,,,,,{}", p.name, q(&p.function))?;
                }
                for pad in &m.pads {
                    let loc = pad.loc.map_or(",,".into(), |(x, y, z)| format!("{x},{y},{z}"));
                    writeln!(o, "{},{},{loc},{},{},{},{},{},{}", p.name, pad.id, pad.bank, q(&pad.io_type),
                        q(&pad.pad_name), q(&pad.function), q(&pad.alt_name), q(&p.function))?;
                }
            }
        }
        ("pins-trace", [f]) => {
            // Decoded values in PDB_ASCII_DUMP order, one per line, for
            // diffing against verify/pins_trace_tokens.py.
            let d = container::Ddb::open(&PathBuf::from(f))?;
            let names: Vec<String> = pdb::parse_class_table(&d.footer)?.into_iter().map(|c| c.name).collect();
            let mut log = Vec::new();
            if names.iter().any(|n| n == "DEV_PIN") {
                pins::parse_pkg(&d, Some(&mut log))?;
            } else {
                pins::parse_pad(&d, Some(&mut log))?;
            }
            println!("{}", log.join("\n"));
        }
        ("export", [dir, die, out]) => {
            export::run(&PathBuf::from(dir), die, &PathBuf::from(out))?;
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
