//! `DYGR_ROUTE_INFO_BODY` (ddb_<die>_routing.ddb): the routing graph.
//!
//! Layout learned from a `PDB_ASCII_DUMP=on` run of quartus_fit, which
//! prints every member name as it is read, plus the accessors in
//! libddb_dygr.so (`DYGR_ROUTE_ELEMENT::get_fanout_incl_purely_redundant_edges`
//! @ 0xe7180: target = template.offsets[k] + element.fanout_edge_base).
//!
//! The graph is stored compactly: every routing element ("node") has an
//! (x, y) tile, a `fanout_edge_base`, and a pointer to a shared
//! `DYGR_ROUTE_ELEMENT_TEMPLATE` that holds the node's type, z/index, wire
//! length, direction and its fanout list *relative to* fanout_edge_base.
//! Identical tiles share templates.

use anyhow::{ensure, Context, Result};

use crate::pdb::{Cursor, Ptr};

/// Bit 31 of a global id marks a routing element; the low 31 bits index
/// `m_route_element_list` (`DYGR_DIE_INFO_BODY::get_node_index_of_id` @ 0xdd280).
pub const ROUTER_ID_BIT: u32 = 0x8000_0000;

#[derive(Debug, Clone, Copy)]
pub struct Element {
    pub x: i16,
    pub y: i16,
    /// Global id that template fanout offsets are relative to.
    pub fanout_edge_base: u32,
    /// Index into `Routing::templates` (template object id - 1).
    pub template: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Template {
    /// `DEV_ELEMENT_ENUM` value (wire/port kind); names live in DEV_ELEMENT_DESC.
    pub element_enum: i16,
    pub num_edges: i16,
    pub z: i16,
    pub index: i16,
    /// Edges at or past this index are electrically redundant duplicates.
    pub first_purely_redundant_edge: i16,
    pub length: i8,
    pub metal_layer: i8,
    pub direction: i8,
    pub fanout_offsets: Vec<i32>,
    pub fanout_to_positions: Vec<i16>,
}

pub struct Routing {
    pub elements: Vec<Element>,
    pub templates: Vec<Template>,
}

impl Routing {
    /// Global ids of node `i`'s fanouts (edges past `first_purely_redundant_edge` included).
    pub fn fanout(&self, i: usize) -> impl Iterator<Item = u32> + '_ {
        let e = &self.elements[i];
        let t = &self.templates[e.template as usize];
        t.fanout_offsets.iter().map(move |&o| e.fanout_edge_base.wrapping_add(o as u32))
    }
}

/// Pointer ids are allocated in the order "new" pointers appear; back
/// references carry `(id - last_id)` masked to 30 bits with bit 30 set so
/// the encoded value stays positive (`PDB_SEGMENT_READER::read_ptr` @ 0x17a80).
struct PtrTable {
    next: u32,
    last: u32,
}

impl PtrTable {
    fn new() -> Self {
        // id 0 is the null pointer.
        PtrTable { next: 1, last: 0 }
    }
    fn resolve(&mut self, p: Ptr) -> Result<u32> {
        let id = match p {
            Ptr::Null => anyhow::bail!("unexpected null template"),
            Ptr::New { .. } => {
                let id = self.next;
                self.next += 1;
                id
            }
            Ptr::Ref { rel } => (self.last.wrapping_add(rel as u32)) & 0x3fff_ffff,
        };
        self.last = id;
        Ok(id)
    }
}

pub fn parse(body: &[u8]) -> Result<Routing> {
    let mut c = Cursor::new(body);
    c.ptr()?.expect_new()?; // the DYGR_ROUTE_INFO_BODY itself

    // Delta fields are relative to the same field of the previously
    // serialized object of the class (`xfr_*_delta`, reference offset at
    // PDB_SEGMENT_BASE+0x190); the first one is relative to a zero buffer.
    let n = c.i32()? as usize;
    let mut tbl = PtrTable::new();
    let mut elements = Vec::with_capacity(n);
    let (mut x, mut y, mut base) = (0i16, 0i16, 0u32);
    for _ in 0..n {
        x = x.wrapping_add(c.i16()?);
        y = y.wrapping_add(c.i16()?);
        base = base.wrapping_add(c.u32()?);
        let id = tbl.resolve(c.ptr()?)?;
        elements.push(Element { x, y, fanout_edge_base: base, template: id - 1 });
    }

    // m_route_element_templates: the templates not referenced from any element.
    let nt = c.i32()? as usize;
    for _ in 0..nt {
        tbl.resolve(c.ptr()?)?;
    }
    let num_templates = (tbl.next - 1) as usize;
    let after_vectors = c.pos();

    // Between here and the template bodies sit lookup tables, block
    // interfaces and block nodes that the graph itself doesn't need. Their
    // schema is not decoded yet, so locate the template bodies instead:
    // they are the last objects in the body and must parse to exactly the
    // end with exactly `num_templates` records.
    let start = find_template_start(body, after_vectors, num_templates)
        .context("could not locate DYGR_ROUTE_ELEMENT_TEMPLATE bodies")?;
    let mut c = Cursor::new(&body[start..]);
    let templates = parse_templates(&mut c, num_templates)?;
    ensure!(c.is_empty(), "templates did not end at end of body");
    Ok(Routing { elements, templates })
}

/// Template bodies, serialized by `PDB_OBJECT_INFO::finish_reading` in id
/// order. Scalar fields are deltas against the previous template; vector
/// elements against the previous element. Restarting each vector from 0
/// reproduces the Quartus trace exactly for magellan4, whether or not
/// element 0 really chains to the previous template like ASM vectors do.
fn parse_templates(c: &mut Cursor, n: usize) -> Result<Vec<Template>> {
    let mut out = Vec::with_capacity(n);
    let mut prev = Template::default();
    for _ in 0..n {
        let mut t = Template {
            element_enum: prev.element_enum.wrapping_add(c.i16()?),
            num_edges: prev.num_edges.wrapping_add(c.i16()?),
            z: prev.z.wrapping_add(c.i16()?),
            index: prev.index.wrapping_add(c.i16()?),
            first_purely_redundant_edge: prev.first_purely_redundant_edge.wrapping_add(c.i16()?),
            // 8-bit bitfields (`xfr_field_v` with width <= 8 reads one byte).
            length: prev.length.wrapping_add(c.i8()?),
            metal_layer: prev.metal_layer.wrapping_add(c.i8()?),
            direction: prev.direction.wrapping_add(c.i8()?),
            ..Default::default()
        };
        let k = c.i32()?;
        ensure!((0..=4096).contains(&k), "implausible fanout count {k}");
        let mut acc = 0i32;
        t.fanout_offsets = (0..k).map(|_| c.i32().map(|d| { acc = acc.wrapping_add(d); acc })).collect::<Result<_>>()?;
        let k2 = c.i32()?;
        ensure!(k2 == k, "fanout_to_positions length {k2} != fanout count {k}");
        let mut acc = 0i16;
        t.fanout_to_positions = (0..k2).map(|_| c.i16().map(|d| { acc = acc.wrapping_add(d); acc })).collect::<Result<_>>()?;
        ensure!(t.num_edges as i32 == k, "m_num_edges {} != fanout count {k}", t.num_edges);
        prev = t.clone();
        out.push(t);
    }
    Ok(out)
}

/// Each template is 17 bytes of scalars plus two length-prefixed arrays, so
/// a wrong start offset almost always fails within a few records; the
/// count + exact-end requirement rules out late starts that happen to sync.
fn find_template_start(body: &[u8], from: usize, n: usize) -> Option<usize> {
    // Every template is at least 25 bytes, which bounds the search window.
    let latest = body.len().checked_sub(n * 25)?;
    (from..=latest).find(|&s| {
        let mut c = Cursor::new(&body[s..]);
        parse_templates_quiet(&mut c, n) && c.is_empty()
    })
}

fn parse_templates_quiet(c: &mut Cursor, n: usize) -> bool {
    (|| -> Result<()> {
        parse_templates(c, n)?;
        Ok(())
    })()
    .is_ok()
}
