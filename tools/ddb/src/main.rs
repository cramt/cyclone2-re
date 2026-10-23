use ddb::{asm, asmdb, container, dtm, export, pdb, pins, routing, rtiming};

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
  ddb asmdb-canon <asmdb.ddb>         mux/arch-group/block summary, see verify/README.md
  ddb pins <devinfo> <die> <package>  package pin -> pad -> IOE x,y,z as CSV
                                      (e.g. magellan4 fbga672)
  ddb pins-trace <pad|pkg ddb>        value stream, see verify/README.md
  ddb dmf <file.dmf.ddb> [names-dir]  every cell delay leaf: node path, key, value
                                      (ps); names-dir from verify/dbs_enum_strings.py
  ddb dmf-trace <file.dmf.ddb>        value stream, see verify/README.md
  ddb route-timing <timing.ddb> <routing.ddb>
                                      per routing element: x, y, type, length, R, C,
                                      driver model, loads (CSV)
  ddb route-timing-trace <timing.ddb> value stream, see verify/README.md"
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
        ("asmdb-canon", [f]) => {
            // Mux bit lists in id order, then arch groups, then block infos,
            // for diffing against verify/asmdb_canon.py | asmdb_semantic.py.
            use std::io::Write;
            let d = container::Ddb::open(&PathBuf::from(f))?;
            let db = asmdb::parse(&d.body)?;
            let mut o = std::io::BufWriter::new(std::io::stdout().lock());
            for m in &db.muxes {
                write!(o, "M")?;
                for b in &m.bits { write!(o, " {b}")?; }
                writeln!(o)?;
            }
            for g in &db.arch_groups {
                let paths: Vec<usize> = g.paths().iter().map(Vec::len).collect();
                writeln!(o, "A {} {} {} {:?}", g.arch_type, g.block_type, g.group_index, paths)?;
            }
            for b in &db.block_infos {
                let n: usize = b.block_lists.values().map(Vec::len).sum();
                writeln!(o, "B {} {} {} {:?}", b.block_type, b.num_ram_cols, n, b.simple_block_lists)?;
            }
            for b in &db.block_infos {
                write!(o, "L")?;
                for v in b.block_lists.values().flatten() { write!(o, " {}", *v as i32)?; }
                writeln!(o)?;
            }
        }
        ("asmdb-json", [f]) => {
            // Whole asmdb as one JSON document, for exploration scripts.
            use serde_json::json;
            let d = container::Ddb::open(&PathBuf::from(f))?;
            let db = asmdb::parse(&d.body)?;
            let id = |m: &Option<asmdb::MuxId>| m.map(|m| m.0);
            let muxes: Vec<_> = db.muxes.iter().map(|m| json!({"bits": m.bits, "kind": format!("{:?}", m.kind)})).collect();
            let groups: Vec<_> = db
                .arch_groups
                .iter()
                .map(|g| {
                    let body = match &g.body {
                        asmdb::ArchBody::Muxed { paths, data, block_info, .. } => json!({
                            "paths": paths.iter().map(|p| p.iter().map(id).collect::<Vec<_>>()).collect::<Vec<_>>(),
                            "data": data.iter().map(|d| json!([d.arch_value, id(&d.aux), id(&d.next_selection)])).collect::<Vec<_>>(),
                            "block_info": block_info,
                        }),
                        asmdb::ArchBody::Dumb { arch_value, bits } => json!({"dumb": arch_value, "bits": bits}),
                        asmdb::ArchBody::Stlmap { paths, .. } => json!({
                            "paths": paths.iter().map(|p| p.iter().map(id).collect::<Vec<_>>()).collect::<Vec<_>>(),
                        }),
                    };
                    json!({"type": g.arch_type, "block": g.block_type, "index": g.group_index, "body": body})
                })
                .collect();
            let blocks: Vec<_> = db.block_infos.iter().map(|b| json!({"type": b.block_type, "cols": b.num_ram_cols, "lists": b.block_lists, "simple": b.simple_block_lists})).collect();
            println!("{}", json!({"muxes": muxes, "groups": groups, "blocks": blocks, "block_info_map": db.block_info_map,
                "default_bits": db.default_bits, "default_mux_settings": db.default_mux_settings, "default_bit_calcs": db.default_bit_calcs}));
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
        ("dmf", [f, rest @ ..]) if rest.len() <= 1 => {
            // One line per leaf: `/`-joined node path, key words, value.
            use std::io::Write;
            let d = container::Ddb::open(&PathBuf::from(f))?;
            let db = dtm::parse(&d, None)?;
            let names = match rest.first() {
                Some(dir) => dtm::Names::load(&PathBuf::from(dir))?,
                None => dtm::Names::default(),
            };
            let mut o = std::io::BufWriter::new(std::io::stdout().lock());
            for (path, leaf) in db.walk() {
                let p: Vec<String> = path
                    .iter()
                    .map(|(e, id)| match e {
                        dtm::Edge::Child => names.node(*id),
                        e => format!("{e:?}:{}", names.node(*id)),
                    })
                    .collect();
                let k: Vec<String> = leaf.key.iter().map(|w| names.wrapped(w)).collect();
                let v = match leaf.value {
                    dtm::Value::Int(v) => v.to_string(),
                    dtm::Value::Float(v) => v.to_string(),
                };
                writeln!(o, "{}\t{}\t{v}", p.join("/"), k.join(" "))?;
            }
        }
        ("dmf-trace", [f]) => {
            // Decoded values in PDB_ASCII_DUMP order, for diffing against
            // verify/dmf_trace_tokens.py.
            let d = container::Ddb::open(&PathBuf::from(f))?;
            let mut log = Vec::new();
            // Print what was read even on failure: the point of divergence
            // is what one wants to see.
            let r = dtm::parse(&d, Some(&mut log));
            println!("{}", log.join("\n"));
            r?;
        }
        ("route-timing", [f, routing_file]) => {
            use std::io::Write;
            let t = rtiming::parse(&container::Ddb::open(&PathBuf::from(f))?.body, None)?;
            let g = routing::parse(&container::Ddb::open(&PathBuf::from(routing_file))?.body)?;
            anyhow::ensure!(t.node_simple_info.len() == g.elements.len(), "timing and routing element counts differ");
            let mut o = std::io::BufWriter::new(std::io::stdout().lock());
            writeln!(o, "node,x,y,element_enum,z,index,length,r_per_um,c_per_um,driver_pos,driver_model,load_base,loads")?;
            let mut out_of_range = 0usize;
            for (i, e) in g.elements.iter().enumerate() {
                let tpl = &g.templates[e.template as usize];
                let si = &t.simple_info[t.node_simple_info[i] as usize];
                let (dp, dm) = si.drivers.first().map_or((-1, -1), |d| (d.pos, d.kind));
                // A node's loads are its fanout edges (inferred), each as
                // position:sram1:sram2.
                let base = t.node_load_base[i] as usize;
                let n = tpl.fanout_offsets.len();
                if base + n > t.loads.len() {
                    out_of_range += 1;
                }
                let loads: Vec<String> = t.loads.get(base..base + n).unwrap_or(&[]).iter()
                    .map(|l| format!("{}:{}:{}", l.position, l.sram1, l.sram2)).collect();
                writeln!(o, "{i},{},{},{},{},{},{},{},{},{dp},{dm},{base},{}", e.x, e.y, tpl.element_enum, tpl.z, tpl.index,
                    si.length, si.resistance.first().unwrap_or(&f32::NAN), si.capacitance.first().unwrap_or(&f32::NAN), loads.join(" "))?;
            }
            if out_of_range > 0 {
                eprintln!("route-timing: {out_of_range} elements' fanout runs past the load list");
            }
        }
        ("route-timing-trace", [f]) => {
            let d = container::Ddb::open(&PathBuf::from(f))?;
            let mut log = Vec::new();
            let r = rtiming::parse(&d.body, Some(&mut log));
            println!("{}", log.join("\n"));
            r?;
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
