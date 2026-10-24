//! The physical netlist: what a placed-and-routed design configures, in
//! device terms only. Frontends (Quartus post-fit netlist + rcf in
//! `quartus`, nextpnr's routed JSON in `nextpnr`) build one; `bits` turns it
//! into CRAM. Nothing here knows where the design came from.
//!
//! Routing is a set of routing-graph edges. A `Pip` can only be made from a
//! pair of nodes the graph connects, so a net can't name a connection the
//! device doesn't have.

use anyhow::Result;
use std::collections::HashMap;

use ddb::device::{Device, Node};

use crate::io::{IoBit, IoDb, PinUse, UnusedPins};
use crate::lab::LabDb;
use crate::logic::Logic;
use crate::m4k::{M4k, M4kDb};
use crate::rbf::{FrameBit, RepBit};
use crate::addr;

/// One routing-graph edge: fanout `edge` of `src`. Private fields: the only
/// way to get one is `Pip::new`, which checks the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pip {
    src: Node,
    dst: Node,
    edge: u16,
}

impl Pip {
    pub fn new(dev: &Device, src: Node, dst: Node) -> Option<Self> {
        dev.edge(src, dst).map(|k| Pip { src, dst, edge: k as u16 })
    }
    pub fn src(self) -> Node {
        self.src
    }
    pub fn dst(self) -> Node {
        self.dst
    }
    /// Fanout position of `dst` in `src` = asm bit group.
    pub fn edge(self) -> usize {
        self.edge as usize
    }
}

#[derive(Debug, Clone)]
pub struct RoutedNet {
    pub name: String,
    pub pips: Vec<Pip>,
}

/// A placed and routed design.
#[derive(Debug)]
pub struct PhysDesign {
    /// LEs, LAB-wide settings and global clock controls.
    pub logic: Logic,
    /// Every used pin's I/O element.
    pub pins: Vec<PinUse>,
    /// What every other user pin does.
    pub unused_pins: UnusedPins,
    /// M4K RAM blocks in use.
    pub m4ks: Vec<M4k>,
    /// The JTAG and serial-flash blocks, when the design uses them.
    pub hard: crate::hard::Hard,
    pub nets: Vec<RoutedNet>,
}

/// Where a bit's value came from, for checking and residual accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    /// ddb asm: a routing edge (value 1, or the encoded value).
    Routing,
    /// ddb asmdb: LUT contents, LE input muxes and LE/LAB settings.
    Logic,
    /// ddb pad/pkg/asmdb: I/O element configuration of every pin.
    Io,
    /// ddb asmdb: M4K RAM block configuration and contents.
    Ram,
    /// ddb asmdb: the JTAG and serial-flash blocks' input muxes.
    Hard,
}

/// The CRAM a physical design fixes. Bits not listed keep the base image's
/// value.
pub struct Bits {
    pub core: Vec<(FrameBit, bool, Source)>,
    /// IOE bits in the replicated frames (`Source::Io`).
    pub rep: Vec<(RepBit, bool)>,
    /// Option bits of the header's ID block to set: (byte, bit).
    pub options: Vec<(usize, u8)>,
}

impl Bits {
    /// Bits two sources set to different values.
    pub fn conflicts(&self) -> Vec<FrameBit> {
        let mut seen: HashMap<FrameBit, bool> = HashMap::new();
        let mut out = Vec::new();
        for &(b, v, _) in &self.core {
            if seen.insert(b, v).is_some_and(|old| old != v) {
                out.push(b);
            }
        }
        out
    }
}

/// The device facts `bits` needs.
pub struct Db {
    pub asmdb: ddb::asmdb::Asmdb,
    pub dev: Device,
    pub lab: LabDb,
    pub io: IoDb,
    pub m4k: M4kDb,
    pub hard: crate::hard::HardDb,
}

impl PhysDesign {
    pub fn bits(&self, db: &Db) -> Result<Bits> {
        let mut core = Vec::new();
        for n in &self.nets {
            for p in &n.pips {
                for b in db.dev.edge_bits(p.src, p.edge()) {
                    // Encoded mux bits (GCLK -> row clock, ~1.5% of all) take
                    // their listed level; plain ones are on when the edge is.
                    let v = if b.use_encoded_setting != 0 { b.is_encoded_bit_high != 0 } else { true };
                    core.push((addr::frame_bit(b.flat_address), v, Source::Routing));
                }
            }
        }
        for (flat, v) in self.logic.bits(&db.lab)? {
            core.push((addr::frame_bit(flat), v, Source::Logic));
        }
        for m in &self.m4ks {
            for (flat, v) in m.bits(&db.m4k)? {
                core.push((addr::frame_bit(flat), v, Source::Ram));
            }
        }
        for flat in db.hard.bits(&self.hard)? {
            core.push((addr::frame_bit(flat), true, Source::Hard));
        }
        // The serial-flash block drives ASDO and nCSO, so they lose the
        // pull-up an unclaimed one has, and it sets an option bit in the
        // header (hard.rs).
        let serial_flash = self.hard.asmi.is_some();
        let options = if serial_flash { vec![crate::hard::ASMI_OPTION] } else { Vec::new() };
        let mut rep = Vec::new();
        for (b, v) in db.io.bits(&self.pins, self.unused_pins, serial_flash)? {
            match b {
                IoBit::Core(b) => core.push((b, v, Source::Io)),
                IoBit::Rep(b) => rep.push((b, v)),
            }
        }
        Ok(Bits { core, rep, options })
    }
}
