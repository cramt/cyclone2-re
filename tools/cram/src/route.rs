//! Routed nets (from an `.rcf`) -> routing-graph edges -> flat CRAM bits.

use anyhow::Result;
use ddb::asm::AsmBit;
use ddb::device::{Device, Node};

use crate::rcf::{Resource, Signal};

/// rcf resource kind -> `DEV_ELEMENT_ENUM` of the routing element. Found by
/// resolving rcf chains against the graph: with these, every consecutive
/// pair of wires in every fuzz design is exactly one fanout edge.
/// `IO_DATAOUT` and LE inputs aren't routing elements: the graph ends at
/// LOCAL_INTERCONNECT, and the last hop into the cell is block config.
pub fn element_enum(kind: &str) -> Option<i16> {
    Some(match kind {
        "LE_BUFFER" => 45,
        "LOCAL_INTERCONNECT" => 276,
        "R4" => 264,
        "R24" => 268,
        "C4" => 269,
        "C16" => 275,
        "CLK_BUFFER" => 289,
        // Global clock path, matched against the graph on the counter
        // design: pin buffer -> clkctrl in (513) .. clkctrl out (515) ->
        // GCLK driver (292) -> quadrant spine `GCLK<n>CS..` (293) -> row
        // clock `RCLK..` (294) -> LAB clock buffer (560). 513 -> 515 is
        // inside the clkctrl block, not a graph edge.
        "CLKBUF_IN" => 513,
        "CLKBUF_OUT" => 515,
        "GLOBAL_CLK_H" => 292,
        "GLOBAL_CLK_V" => 293,
        "LAB_CLK" => 294,
        "BLK_CLK_BUF" => 560,
        // An LE's direct feedback into its own LAB: index = LE number.
        // Edge-less in the graph; the LE input mux selects it.
        "LOCAL_LINE" => 43,
        _ => return None,
    })
}

#[derive(Debug, Clone)]
pub struct Hop {
    pub src: Node,
    pub dst: Node,
    /// Fanout position of `dst` in `src` = ASM bit group.
    pub edge: usize,
}

#[derive(Debug, Default)]
pub struct Resolved {
    pub hops: Vec<Hop>,
    /// Resources that aren't routing elements or didn't resolve.
    pub skipped: Vec<Resource>,
}

/// Each resource's driver is the latest earlier resource of the same net
/// with an edge to it, so branching trees resolve without knowing the rcf
/// tree syntax.
pub fn resolve(dev: &Device, sig: &Signal) -> Resolved {
    let mut out = Resolved::default();
    let mut seen: Vec<Node> = Vec::new();
    for r in &sig.resources {
        let Some(e) = element_enum(&r.kind) else {
            out.skipped.push(r.clone());
            continue;
        };
        let cands: Vec<Node> =
            dev.at(r.x, r.y, r.z, r.index).iter().copied().filter(|&n| dev.element_enum(n) == e).collect();
        let [node] = cands[..] else {
            out.skipped.push(r.clone());
            continue;
        };
        if let Some((src, k)) = seen.iter().rev().find_map(|&s| dev.edge(s, node).map(|k| (s, k))) {
            out.hops.push(Hop { src, dst: node, edge: k });
        } else if !seen.is_empty() {
            out.skipped.push(r.clone());
        }
        seen.push(node);
    }
    out
}

/// Every CRAM bit the routed nets set.
pub fn bits<'d>(dev: &'d Device, sigs: &[Signal]) -> Result<(Vec<&'d AsmBit>, Vec<Resource>)> {
    let mut bits = Vec::new();
    let mut skipped = Vec::new();
    for s in sigs {
        let r = resolve(dev, s);
        for h in &r.hops {
            bits.extend(dev.edge_bits(h.src, h.edge));
        }
        skipped.extend(r.skipped);
    }
    Ok((bits, skipped))
}
