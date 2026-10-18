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
    let mut nodes: Vec<Option<Node>> = Vec::new();
    for (i, r) in sig.resources.iter().enumerate() {
        let node = element_enum(&r.kind).and_then(|e| {
            let cands: Vec<Node> =
                dev.at(r.x, r.y, r.z, r.index).iter().copied().filter(|&n| dev.element_enum(n) == e).collect();
            match cands[..] {
                [n] => Some(n),
                _ => None,
            }
        });
        nodes.push(node);
        let Some(node) = node else {
            out.skipped.push(r.clone());
            continue;
        };
        // The rcf tree names each resource's parent (previous line or a
        // branch_point label); fall back to the latest earlier resource with
        // an edge for anything the tree doesn't pin down.
        let parent = sig.parents.get(i).copied().flatten().and_then(|p| nodes[p]).and_then(|p| dev.edge(p, node).map(|k| (p, k)));
        let hop = parent.or_else(|| nodes[..i].iter().rev().flatten().find_map(|&s| dev.edge(s, node).map(|k| (s, k))));
        match hop {
            Some((src, k)) => out.hops.push(Hop { src, dst: node, edge: k }),
            None if i > 0 => out.skipped.push(r.clone()),
            None => {}
        }
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
