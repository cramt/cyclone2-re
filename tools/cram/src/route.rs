//! Routed nets (from an `.rcf`) -> routing-graph edges -> flat CRAM bits.

use chipsmith_cyclone2::bitstream::device::{AsmBit, Device, Node};
use chipsmith_cyclone2::bitstream::phys::{Pip, RoutedNet};

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
        // Input pin into the core: index 0 has 18 fanouts, index 1 has 36
        // (I/O fuzz designs only ever used index 1). Matched against the
        // graph the same way as the rest.
        "IO_DATAIN" => 301,
        // Top/bottom I/O clock spine (`BIOCLK..`/`TIOCLK..`): GCLK -> I/O
        // row clock buffers, like LAB_CLK for LABs.
        "TB_IO_CLK" => 298,
        // M4K data out onto the routing (M4K fuzz designs: every output's
        // first hop is an edge from this element).
        "M4K_BUFFER" => 278,
        // The JTAG and serial-flash (ASMI) hard blocks' outputs into the
        // core, at X6 Y20-22: matched against their rcf chains like the rest
        // (hard-block designs, README "JTAG and serial-flash blocks").
        "JTAG_TMSUTAP_PIN" => 361,
        "JTAG_TCKUTAP_PIN" => 362,
        "JTAG_TDIUTAP_PIN" => 363,
        "JTAG_SHIFTUSER_PIN" => 364,
        "JTAG_CLKDRUSER_PIN" => 365,
        "JTAG_UPDATEUSER_PIN" => 366,
        "JTAG_RUNIDLEUSER_PIN" => 367,
        "JTAG_USR1USER_PIN" => 368,
        "ASMI_DATAOUT" => 474,
        // Multiplier block (X39): product bits onto the routing, and the
        // five control inputs (clock, enable, clear, signa, signb) fed from
        // the block's local lines (multiplier fuzz designs).
        "DSP_BUFFER" => 280,
        "DSP_CONTROL_INPUT" => 374,
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

/// The routed nets as graph edges, plus the rcf resources that aren't
/// graph edges (block-level hops the logic and I/O models cover).
pub fn nets(dev: &Device, sigs: &[Signal]) -> (Vec<RoutedNet>, Vec<Resource>) {
    let mut nets = Vec::new();
    let mut skipped = Vec::new();
    for s in sigs {
        let r = resolve(dev, s);
        let pips = r.hops.iter().map(|h| Pip::new(dev, h.src, h.dst).expect("resolve only yields graph edges")).collect();
        nets.push(RoutedNet { name: s.name.clone(), pips });
        skipped.extend(r.skipped);
    }
    (nets, skipped)
}

/// Every CRAM bit the routed nets set.
pub fn bits<'d>(dev: &'d Device, sigs: &[Signal]) -> (Vec<&'d AsmBit>, Vec<Resource>) {
    let (nets, skipped) = nets(dev, sigs);
    let bits = nets.iter().flat_map(|n| &n.pips).flat_map(|p| dev.edge_bits(p.src(), p.edge())).collect();
    (bits, skipped)
}
