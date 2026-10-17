use ddb::{asm, container, export, pdb, routing};

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
  ddb asm-canon <asm.ddb>"
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
