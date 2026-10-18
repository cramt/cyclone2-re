//! One die's routing graph joined with its routing config bits: the device
//! side of "which CRAM bits does this routed net set".

use anyhow::{ensure, Context, Result};
use std::collections::HashMap;
use std::path::Path;

use crate::{asm, container::Ddb, routing};

/// Index into the die's routing element list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Node(pub u32);

pub struct Device {
    pub routing: routing::Routing,
    pub asm: Vec<asm::AsmNode>,
    by_loc: HashMap<(i16, i16, i16, i16), Vec<Node>>,
}

impl Device {
    pub fn load(devinfo: &Path, die: &str) -> Result<Self> {
        let open = |kind: &str| -> Result<Ddb> {
            let p = devinfo.join(format!("ddb_{die}_{kind}.ddb"));
            Ddb::open(&p).with_context(|| format!("opening {}", p.display()))
        };
        Self::new(routing::parse(&open("routing")?.body)?, asm::parse(&open("asm")?.body)?)
    }

    /// The graph and its bits from wherever they were stored (Quartus'
    /// files, or the `cram` crate's own database).
    pub fn new(routing: routing::Routing, asm: Vec<asm::AsmNode>) -> Result<Self> {
        ensure!(asm.len() == routing.elements.len(), "ASM node count != routing element count");
        let mut by_loc: HashMap<_, Vec<Node>> = HashMap::new();
        for (i, e) in routing.elements.iter().enumerate() {
            let t = &routing.templates[e.template as usize];
            by_loc.entry((e.x, e.y, t.z, t.index)).or_default().push(Node(i as u32));
        }
        Ok(Device { routing, asm, by_loc })
    }

    pub fn element_enum(&self, n: Node) -> i16 {
        self.template(n).element_enum
    }

    fn template(&self, n: Node) -> &routing::Template {
        &self.routing.templates[self.routing.elements[n.0 as usize].template as usize]
    }

    /// All routing elements at tile (x, y) with this z and index; several
    /// element types share each coordinate.
    pub fn at(&self, x: i16, y: i16, z: i16, index: i16) -> &[Node] {
        self.by_loc.get(&(x, y, z, index)).map_or(&[], Vec::as_slice)
    }

    /// Position of `dst` in `src`'s fanout list, which is also the ASM bit
    /// group that enables the edge (`get_destination_bit_group`).
    pub fn edge(&self, src: Node, dst: Node) -> Option<usize> {
        let gid = dst.0 | routing::ROUTER_ID_BIT;
        self.routing.fanout(src.0 as usize).position(|g| g == gid)
    }

    /// CRAM bits that turn on edge `k` of `src`.
    pub fn edge_bits(&self, src: Node, k: usize) -> &[asm::AsmBit] {
        self.asm[src.0 as usize].group(k)
    }
}
