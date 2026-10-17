//! Dump the decoded routing graph, wire names and routing CRAM bits to
//! CSV so later phases (and people) can use them without this crate.

use anyhow::{ensure, Context, Result};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use crate::{asm, container::Ddb, icname, routing};

pub fn run(devinfo: &Path, die: &str, out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    let open = |kind: &str| -> Result<Ddb> {
        let p = devinfo.join(format!("ddb_{die}_{kind}.ddb"));
        Ddb::open(&p).with_context(|| format!("opening {}", p.display()))
    };
    let csv = |name: &str| -> Result<std::io::BufWriter<std::fs::File>> {
        Ok(std::io::BufWriter::new(std::fs::File::create(out.join(name))?))
    };

    let ic = icname::parse(&open("icname")?.body)?;
    let r = routing::parse(&open("routing")?.body)?;
    let a = asm::parse(&open("asm")?.body)?;
    let n = r.elements.len();
    ensure!(a.len() == n, "{} ASM nodes for {n} routing elements", a.len());

    let index_of = |gid: u32| -> Option<usize> {
        (gid & routing::ROUTER_ID_BIT != 0).then_some((gid & !routing::ROUTER_ID_BIT) as usize).filter(|&i| i < n)
    };

    let mut names: HashMap<usize, (&str, &str)> = HashMap::new();
    let mut f = csv("icname.csv")?;
    writeln!(f, "gid,node,name,sch_name")?;
    ensure!(ic.names.len() == ic.sch_names.len(), "name / sch_name maps differ in size");
    for ((gid, name), (gid2, sch)) in ic.names.iter().zip(&ic.sch_names) {
        ensure!(gid == gid2, "name and sch_name keys diverge at {gid:#x}");
        let node = index_of(*gid).with_context(|| format!("icname key {gid:#x} is not a routing id"))?;
        names.insert(node, (name, sch));
        writeln!(f, "{gid:#010x},{node},{name},{sch}")?;
    }

    // Fan-in is the inverse of the stored fanout lists.
    let mut fanin_count = vec![0u32; n];
    let mut non_router_targets = 0usize;
    let mut e = csv("edges.csv")?;
    writeln!(e, "dst,src,src_edge,redundant,asm_bits")?;
    let mut edges = 0usize;
    let mut groups_eq_edges = 0usize;
    let mut groups_eq_nonredundant = 0usize;
    let mut asm_bits_total = 0usize;
    for (src, node) in a.iter().enumerate() {
        let t = &r.templates[r.elements[src].template as usize];
        let groups = node.start_bit_index.len();
        groups_eq_edges += (groups == t.fanout_offsets.len()) as usize;
        groups_eq_nonredundant += (groups == t.first_purely_redundant_edge as usize) as usize;
        asm_bits_total += node.bits.len();
        for (k, gid) in r.fanout(src).enumerate() {
            edges += 1;
            let Some(dst) = index_of(gid) else {
                non_router_targets += 1;
                continue;
            };
            fanin_count[dst] += 1;
            let bits = if k < groups {
                node.group(k).iter().map(fmt_bit).collect::<Vec<_>>().join(" ")
            } else {
                String::new()
            };
            let redundant = k >= t.first_purely_redundant_edge as usize;
            writeln!(e, "{dst},{src},{k},{},{bits}", redundant as u8)?;
        }
    }

    let mut f = csv("nodes.csv")?;
    writeln!(f, "node,gid,x,y,z,index,element_enum,length,metal_layer,direction,fanout,first_redundant_edge,fanin,asm_groups,asm_bits,template,name")?;
    for (i, el) in r.elements.iter().enumerate() {
        let t = &r.templates[el.template as usize];
        let name = names.get(&i).map_or("", |n| n.0);
        writeln!(
            f,
            "{i},{:#010x},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{name}",
            i as u32 | routing::ROUTER_ID_BIT,
            el.x, el.y, t.z, t.index, t.element_enum, t.length, t.metal_layer, t.direction,
            t.fanout_offsets.len(), t.first_purely_redundant_edge, fanin_count[i],
            a[i].start_bit_index.len(), a[i].bits.len(), el.template
        )?;
    }

    let summary = serde_json::json!({
        "die": die,
        "routing_elements": n,
        "element_templates": r.templates.len(),
        "fanout_edges": edges,
        "fanout_edges_to_non_router_ids": non_router_targets,
        "icname_entries": ic.names.len(),
        "asm_nodes": a.len(),
        "asm_bits": asm_bits_total,
        "asm_nodes_with_groups_eq_fanout": groups_eq_edges,
        "asm_nodes_with_groups_eq_first_redundant_edge": groups_eq_nonredundant,
    });
    std::fs::write(out.join("summary.json"), serde_json::to_string_pretty(&summary)?)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

/// `addr` for a plain bit, with a letter per set flag. The flags are 1-bit
/// signed bitfields in Quartus, so "true" is serialized as -1 (0xff).
fn fmt_bit(b: &asm::AsmBit) -> String {
    let mut s = b.flat_address.to_string();
    for (flag, tag) in [
        (b.use_encoded_setting, 'e'),
        (b.is_encoded_bit_high, 'h'),
        (b.is_strangely_encoded, 's'),
        (b.is_cff_bit, 'c'),
    ] {
        if flag != 0 {
            s.push(tag);
        }
    }
    s
}
