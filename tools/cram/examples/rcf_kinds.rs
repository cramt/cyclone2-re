//! Which routing element enum is an rcf resource kind? For each resource the
//! routing table doesn't know, list the elements at its (x, y, z, index) and
//! whether each has a graph edge from the previous resource / to the next.
//! (debug aid for adding rows to `route::element_enum`)
//!
//!     cargo run --release --example rcf_kinds -- <design dir>...
use std::collections::BTreeMap;

fn main() -> anyhow::Result<()> {
    let devinfo = std::path::PathBuf::from(std::env::var("HOME")?).join("altera/13.0sp1/quartus/common/devinfo/cycloneii");
    let dev = ddb::device::load(&devinfo, "magellan4")?;
    // kind -> enum -> (seen, edge from prev, edge to next)
    let mut tally: BTreeMap<String, BTreeMap<i16, (usize, usize, usize)>> = BTreeMap::new();
    for d in std::env::args().skip(1) {
        let sigs = cram::rcf::read(&std::path::Path::new(&d).join("build/top.rcf"))?;
        for s in &sigs {
            let node_of = |r: &cram::rcf::Resource| -> Option<ddb::device::Node> {
                let e = cram::route::element_enum(&r.kind)?;
                dev.at(r.x, r.y, r.z, r.index).iter().copied().find(|&n| dev.element_enum(n) == e)
            };
            for (i, r) in s.resources.iter().enumerate() {
                if cram::route::element_enum(&r.kind).is_some() {
                    continue;
                }
                let prev = s.parents[i].and_then(|p| node_of(&s.resources[p]));
                let next = s.resources.iter().enumerate().skip(i + 1).find(|(j, _)| s.parents[*j] == Some(i)).and_then(|(_, r)| node_of(r));
                for &n in dev.at(r.x, r.y, r.z, r.index) {
                    let t = tally.entry(r.kind.clone()).or_default().entry(dev.element_enum(n)).or_default();
                    t.0 += 1;
                    if prev.is_some_and(|p| dev.edge(p, n).is_some()) {
                        t.1 += 1;
                    }
                    if next.is_some_and(|x| dev.edge(n, x).is_some()) {
                        t.2 += 1;
                    }
                }
            }
        }
    }
    for (k, m) in tally {
        println!("{k}: {m:?}  (enum: (at loc, edge from prev, edge to next))");
    }
    Ok(())
}
