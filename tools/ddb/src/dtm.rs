//! `DTM` delay model files (`ddb_<die>-<speed>.dmf.ddb`, libddb_dtm.so):
//! the cell delays of one speed grade / corner. LUT input to output, carry
//! chain, register tco/tsu/th, IOE paths, clock buffers, M4K and multiplier
//! blocks, plus a few fixed interconnect delays (the clock network,
//! `IC_TABLE`). Routing wire delays are *not* here; Quartus computes those
//! per net from `ddb_<die>_timing.ddb` and the NLSPC switch models (see
//! docs/timing-model.md).
//!
//! The file is a tree of `DTM_NODE`s. Each node has an id (`DTM_NODE_ENUM`,
//! e.g. `LE_COMB`, `P2P`, `RC_RISE`), child nodes, and optionally a balanced
//! binary tree of leaves (`DTM_TREE_INT_DATA` / `_FLOAT_DATA`). A leaf has a
//! key of up to three *wrapped* enums and a value; integer values are
//! picoseconds. `DTM_P2P_NODE` adds four children for the in->out,
//! in->buried, buried->out and buried->buried point-to-point delays.
//!
//! Format notes (checked against the `PDB_ASCII_DUMP` trace with `ddb
//! dmf-trace` and verify/dmf_trace_tokens.py):
//!
//! - Three pointer tables: nodes (every `DTM_*_NODE` class), tree data
//!   (`DTM_TREE_*DATA`) and keys. They drain in that order: every node, then
//!   every leaf, then every key, each in id order (the LIFO rule of asmdb).
//! - Node subclasses other than `DTM_P2P_NODE` add no members; the trace
//!   prints them as `DTM_NODE`.
//! - `m_version`, `m_bin`, `m_balance_factor` and `m_num_wrapper` are i16
//!   deltas against the previous object of the base class; `m_int_data`
//!   against the previous `DTM_TREE_INT_DATA` (float leaves don't break the
//!   chain); `m_enum_id` against the previous wrapper.
//! - `m_description` is an i32 exact length plus bytes (no NUL stored).

use anyhow::{bail, ensure, Context, Result};
use std::collections::HashMap;

use crate::container::Ddb;
use crate::pdb::{Cursor, Ptr};

/// `DTM_ENUM_WRAPPER::m_enum_type`: which enum `m_enum_id` belongs to
/// (`DTM_ENUM_WRAPPER::get_string`, libddb_dtm.so @ 0x2d65e). For types 2-6
/// the stored id is the enum value + 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum EnumType {
    None,
    /// `DTM_ENUM` (`DBS_DTM_ENUM_STRING` in libdb_dbs.so): parameter names
    /// such as `TCO`, `TSU`, `CHAIN_BASE`, `LVTTL`.
    Dtm,
    /// `DB_BURIED_PORT_TYPE`, e.g. `FF`.
    BuriedPort,
    /// `DB_INPUT_PORT_TYPE`, e.g. `DATAA`, `CIN`, `CLK`.
    InputPort,
    /// `DB_OUTPUT_PORT_TYPE`, e.g. `COMBOUT`, `COUT`, `REGOUT`.
    OutputPort,
    /// `CDB_RE_TYPE`: routing element types (`GLOBAL_CLK_H`, `LAB_CLK`, ...).
    ReType,
    /// `DEV_IO_STANDARD_ENUM`.
    IoStandard,
    /// A plain integer index (`get_string` has no name for it).
    Int,
}

impl EnumType {
    fn from_raw(v: i32) -> Result<Self> {
        Ok(match v {
            0 => EnumType::None,
            1 => EnumType::Dtm,
            2 => EnumType::BuriedPort,
            3 => EnumType::InputPort,
            4 => EnumType::OutputPort,
            5 => EnumType::ReType,
            6 => EnumType::IoStandard,
            7 => EnumType::Int,
            v => bail!("dmf: unknown DTM_ENUM_WRAPPER type {v}"),
        })
    }
    /// The value of the underlying enum (the stored id is off by one for
    /// the port, RE and I/O standard enums).
    pub fn value(self, id: i32) -> i32 {
        match self {
            EnumType::BuriedPort | EnumType::InputPort | EnumType::OutputPort | EnumType::ReType | EnumType::IoStandard => {
                id - 1
            }
            _ => id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Wrapped {
    pub kind: EnumType,
    /// Already converted with `EnumType::value`.
    pub value: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub enum Value {
    /// Picoseconds for delays; some leaves are other integers (voltages in
    /// mV, temperatures, scale factors in 1/1000).
    Int(i32),
    Float(f64),
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Leaf {
    pub key: Vec<Wrapped>,
    pub value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Edge {
    Child,
    InOut,
    InBuried,
    BuriedOut,
    BuriedBuried,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Node {
    /// `DTM_NODE_ENUM` (`DBS_DTM_NODE_ENUM_STRING`).
    pub id: i32,
    pub description: Option<String>,
    pub children: Vec<(Edge, usize)>,
    /// `m_linked_node`: shares another node's data. Not seen set in
    /// Cyclone II files.
    pub linked: Option<usize>,
    /// In-order walk of the node's leaf tree.
    pub leaves: Vec<Leaf>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Dmf {
    /// Index 0 is the root.
    pub nodes: Vec<Node>,
}

impl Dmf {
    /// Every leaf with its node path from the root (root excluded).
    pub fn walk(&self) -> Vec<(Vec<(Edge, i32)>, &Leaf)> {
        let mut out = Vec::new();
        let mut stack = vec![(0usize, Vec::new())];
        while let Some((n, path)) = stack.pop() {
            let node = &self.nodes[n];
            for l in &node.leaves {
                out.push((path.clone(), l));
            }
            for &(e, c) in node.children.iter().rev() {
                let mut p = path.clone();
                p.push((e, self.nodes[c].id));
                stack.push((c, p));
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Table {
    Node,
    Data,
    Key,
}

struct Rd<'a> {
    c: Cursor<'a>,
    log: Option<&'a mut Vec<String>>,
    /// Class id of each object per table (1-based ids), last id resolved.
    tables: HashMap<Table, (Vec<u16>, u32)>,
    classes: HashMap<u16, String>,
    prev_version: i16,
    prev_bin: i16,
    prev_balance: i16,
    prev_int: i32,
    prev_enum_id: i32,
    prev_num_wrapper: i16,
}

impl<'a> Rd<'a> {
    fn emit(&mut self, s: impl FnOnce() -> String) {
        if let Some(l) = self.log.as_deref_mut() {
            l.push(s());
        }
    }
    fn i32(&mut self) -> Result<i32> {
        let v = self.c.i32()?;
        self.emit(|| format!("{v:08X}"));
        Ok(v)
    }
    fn count(&mut self) -> Result<usize> {
        let n = self.i32()?;
        ensure!((0..1 << 24).contains(&n), "dmf: bad count {n} at {:#x}", self.c.pos() - 4);
        Ok(n as usize)
    }
    fn delta16(&mut self, prev: i16) -> Result<i16> {
        let v = prev.wrapping_add(self.c.i16()?);
        self.emit(|| format!("{v:04X}"));
        Ok(v)
    }
    fn table_of(&self, class_id: u16) -> Result<Table> {
        let name = self.classes.get(&class_id).with_context(|| format!("dmf: class id {class_id} not in footer"))?;
        Ok(match name.as_str() {
            "DTM_TREE_INT_DATA" | "DTM_TREE_FLOAT_DATA" | "DTM_TREE_DATA" => Table::Data,
            "DTM_TREE_DATA_KEY" => Table::Key,
            n if n.starts_with("DTM_") && n.ends_with("_NODE") => Table::Node,
            n => bail!("dmf: no pointer table for class {n}"),
        })
    }
    fn ptr(&mut self, want: Table) -> Result<Option<u32>> {
        let raw = self.i32()?;
        let p = Ptr::decode(raw);
        let t = match p {
            Ptr::Null => return Ok(None),
            Ptr::New { class_id } => {
                let t = self.table_of(class_id)?;
                ensure!(t == want, "dmf: new {:?} object in a {want:?} member", t);
                let s = self.tables.entry(t).or_default();
                s.0.push(class_id);
                s.1 = s.0.len() as u32;
                return Ok(Some(s.1));
            }
            Ptr::Ref { .. } => want,
        };
        let Ptr::Ref { rel } = p else { unreachable!() };
        let s = self.tables.entry(t).or_default();
        let id = s.1.wrapping_add(rel as u32) & 0x3fff_ffff;
        ensure!(id >= 1 && id as usize <= s.0.len(), "dmf: {t:?} back-reference to unknown id {id}");
        s.1 = id;
        Ok(Some(id))
    }
    /// The trace prints node subclasses without members of their own as
    /// `DTM_NODE`.
    fn begin(&mut self, class: u16) {
        let mut name = self.classes.get(&class).cloned().unwrap_or_default();
        if name.ends_with("_NODE") && name != "DTM_P2P_NODE" {
            name = "DTM_NODE".into();
        }
        self.emit(|| format!("@{name}"));
    }

    fn node(&mut self, is_p2p: bool) -> Result<RawNode> {
        let mut children = Vec::new();
        for _ in 0..self.count()? {
            if let Some(id) = self.ptr(Table::Node)? {
                children.push((Edge::Child, id));
            }
        }
        let id = self.i32()?;
        let writable = self.c.u8()?;
        self.emit(|| format!("{writable:02X}"));
        let description = self.string()?;
        self.prev_version = self.delta16(self.prev_version)?;
        self.prev_bin = self.delta16(self.prev_bin)?;
        let data_root = self.ptr(Table::Data)?;
        let linked = self.ptr(Table::Node)?;
        if is_p2p {
            for e in [Edge::InOut, Edge::InBuried, Edge::BuriedOut, Edge::BuriedBuried] {
                if let Some(id) = self.ptr(Table::Node)? {
                    children.push((e, id));
                }
            }
        }
        Ok(RawNode { id, description, children, data_root, linked })
    }
    /// i32 exact length, then the bytes; 0 = none. The trace prints a NUL
    /// that is not stored.
    fn string(&mut self) -> Result<Option<String>> {
        let s = self.c.std_string_opt()?;
        let n = s.as_ref().map_or(0, String::len);
        self.emit(|| format!("{n:08X}"));
        if let Some(s) = &s {
            self.emit(|| format!("\"{s}\""));
        }
        Ok(s)
    }
}

/// A node as stored: 1-based ids of children, leaf tree root, linked node.
struct RawNode {
    id: i32,
    description: Option<String>,
    children: Vec<(Edge, u32)>,
    data_root: Option<u32>,
    linked: Option<u32>,
}

/// A leaf as stored: tree links, key id, value.
struct RawLeaf {
    left: Option<u32>,
    right: Option<u32>,
    key: u32,
    value: Value,
}

pub fn parse(d: &Ddb, log: Option<&mut Vec<String>>) -> Result<Dmf> {
    let classes: HashMap<u16, String> =
        crate::pdb::parse_class_table(&d.footer)?.into_iter().map(|c| (c.class_id as u16, c.name)).collect();
    let cid = |name: &str| {
        classes.iter().find(|(_, n)| *n == name).map(|(i, _)| *i).with_context(|| format!("dmf: no class {name}"))
    };
    let (p2p, int_data, float_data) = (cid("DTM_P2P_NODE")?, cid("DTM_TREE_INT_DATA")?, cid("DTM_TREE_FLOAT_DATA")?);
    let mut r = Rd {
        c: Cursor::new(&d.body),
        log,
        tables: HashMap::new(),
        classes: classes.clone(),
        prev_version: 0,
        prev_bin: 0,
        prev_balance: 0,
        prev_int: 0,
        prev_enum_id: 0,
        prev_num_wrapper: 0,
    };

    // The root DTM_NODE is the inline top-level object and takes node id 1.
    let root_class = r.c.ptr()?.expect_new()?;
    r.emit(|| "FFFFFFFF".into());
    ensure!(r.table_of(root_class)? == Table::Node, "dmf: body doesn't start with a DTM_NODE");
    r.tables.entry(Table::Node).or_default().0.push(root_class);
    r.tables.get_mut(&Table::Node).unwrap().1 = 1;

    // Nodes, in id order (the root's body is inline at offset 4).
    let mut raw_nodes = Vec::new();
    while raw_nodes.len() < r.tables[&Table::Node].0.len() {
        let class = r.tables[&Table::Node].0[raw_nodes.len()];
        r.begin(class);
        let n = r.node(class == p2p).with_context(|| format!("dmf: node #{}", raw_nodes.len() + 1))?;
        raw_nodes.push(n);
    }
    // Then leaves.
    let mut leaves = Vec::new();
    while leaves.len() < r.tables.get(&Table::Data).map_or(0, |t| t.0.len()) {
        let class = r.tables[&Table::Data].0[leaves.len()];
        r.begin(class);
        let left = r.ptr(Table::Data)?;
        let right = r.ptr(Table::Data)?;
        let key = r.ptr(Table::Key)?.context("dmf: leaf without a key")?;
        r.prev_balance = r.delta16(r.prev_balance)?;
        let value = if class == int_data {
            r.prev_int = r.prev_int.wrapping_add(r.c.i32()?);
            let v = r.prev_int;
            r.emit(|| format!("{v:08X}"));
            Value::Int(v)
        } else if class == float_data {
            let v = f64::from_le_bytes(r.c.bytes(8)?.try_into().unwrap());
            r.emit(|| fmt_e(v));
            Value::Float(v)
        } else {
            bail!("dmf: leaf of class {class}");
        };
        leaves.push(RawLeaf { left, right, key, value });
    }
    // Then keys.
    let mut keys = Vec::new();
    while keys.len() < r.tables.get(&Table::Key).map_or(0, |t| t.0.len()) {
        let class = r.tables[&Table::Key].0[keys.len()];
        r.begin(class);
        let n = r.count()?;
        let mut k = Vec::with_capacity(n);
        for _ in 0..n {
            let kind = EnumType::from_raw(r.i32()?)?;
            r.prev_enum_id = r.prev_enum_id.wrapping_add(r.c.i32()?);
            let id = r.prev_enum_id;
            r.emit(|| format!("{id:08X}"));
            k.push(Wrapped { kind, value: kind.value(id) });
        }
        r.prev_num_wrapper = r.delta16(r.prev_num_wrapper)?;
        ensure!(r.prev_num_wrapper as usize == n, "dmf: m_num_wrapper {} != {n} wrappers", r.prev_num_wrapper);
        keys.push(k);
    }
    ensure!(r.c.is_empty(), "dmf: {} trailing bytes", r.c.remaining());

    let mut nodes = Vec::with_capacity(raw_nodes.len());
    for RawNode { id, description, children, data_root: root, linked } in raw_nodes {
        let mut out = Vec::new();
        // In-order walk = ascending key order (the tree is an AVL tree
        // sorted by `DTM_TREE_DATA_KEY::compare_key`).
        let mut stack = Vec::new();
        let mut cur = root;
        while cur.is_some() || !stack.is_empty() {
            while let Some(i) = cur {
                stack.push(i);
                cur = leaves[i as usize - 1].left;
            }
            let i = stack.pop().unwrap();
            let l = &leaves[i as usize - 1];
            out.push(Leaf { key: keys[l.key as usize - 1].clone(), value: l.value });
            cur = l.right;
        }
        nodes.push(Node {
            id,
            description,
            children: children.into_iter().map(|(e, c)| (e, c as usize - 1)).collect(),
            linked: linked.map(|l| l as usize - 1),
            leaves: out,
        });
    }
    Ok(Dmf { nodes })
}

/// C's `%E`: six decimals and a signed two-digit exponent, as the trace
/// prints floats.
fn fmt_e(v: f64) -> String {
    let s = format!("{v:.6E}");
    let (m, e) = s.split_once('E').unwrap();
    let e: i32 = e.parse().unwrap();
    format!("{m}E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
}

/// Enum names for printing. Quartus keeps them in libdb_dbs.so
/// (`DBS_DTM_ENUM_STRING` and friends); verify/dbs_enum_strings.py extracts
/// them into a directory of `<value> <name>` text files, which stays out of
/// the repo like every other Quartus-derived table.
#[derive(Default)]
pub struct Names {
    tables: HashMap<&'static str, HashMap<i32, String>>,
}

impl Names {
    pub const FILES: [&'static str; 6] =
        ["dtm_enum", "dtm_node_enum", "cdb_re_type", "db_input_port", "db_output_port", "db_buried_port"];

    pub fn load(dir: &std::path::Path) -> Result<Self> {
        let mut tables = HashMap::new();
        for f in Self::FILES {
            let p = dir.join(format!("{f}.txt"));
            let text = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
            let t = text
                .lines()
                .filter_map(|l| {
                    let (i, n) = l.split_once(' ')?;
                    Some((i.parse().ok()?, n.trim().to_string()))
                })
                .collect();
            tables.insert(f, t);
        }
        Ok(Names { tables })
    }
    fn get(&self, table: &'static str, v: i32) -> Option<&str> {
        self.tables.get(table)?.get(&v).map(String::as_str)
    }
    pub fn node(&self, id: i32) -> String {
        self.get("dtm_node_enum", id).map_or_else(|| format!("node{id}"), str::to_string)
    }
    pub fn wrapped(&self, w: &Wrapped) -> String {
        let (table, prefix) = match w.kind {
            EnumType::Dtm => ("dtm_enum", ""),
            EnumType::BuriedPort => ("db_buried_port", "B:"),
            EnumType::InputPort => ("db_input_port", "I:"),
            EnumType::OutputPort => ("db_output_port", "O:"),
            EnumType::ReType => ("cdb_re_type", "RE:"),
            EnumType::IoStandard => return format!("IOSTD:{}", w.value),
            EnumType::Int => return format!("#{}", w.value),
            EnumType::None => return "-".into(),
        };
        match self.get(table, w.value) {
            Some(n) => format!("{prefix}{n}"),
            None => format!("{prefix}{:?}{}", w.kind, w.value),
        }
    }
}
