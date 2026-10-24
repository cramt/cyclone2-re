//! `ASMDB_DIE` (ddb_<die>_asmdb.ddb, libdb_asmdb.so): the block-level
//! configuration tables. Each *arch group* is one named feature of one block
//! type (`LE_LUT_INPUT` of a LAB, `IO_PIN_MODE` of an IOE, ...) and lists,
//! per instance inside the block (e.g. per LE), the *muxes* that implement
//! it. A mux is a list of bit positions relative to the block's base flat
//! CRAM address; `ASMDB_BLOCK_INFO` lists every block instance's base.
//!
//! Format notes (all checked against Quartus's PDB_ASCII_DUMP of this file,
//! see verify/asmdb_canon.py and `ddb asmdb-canon`):
//!
//! - Deferred object bodies carry no class tag. The reader knows each
//!   pending object's class from the `new` pointer that created it, and
//!   writes pointer tables LIFO: the most recently created table with
//!   pending objects is drained next, in id order. A table that gets new
//!   objects after it was drained goes back on top.
//! - Strings are an i32 exact length plus bytes (no NUL counted, unlike
//!   the routing files' c_str).
//! - Arch groups share one pointer table (`ARCH_GROUP_*`), all mux classes
//!   another, `DEFAULT_MUX_SETTING` and `DEFAULT_BIT_CALC` a third.
//! - `m_mux_bits` element 0 is a delta against element 0 of the previous
//!   mux of any class (they share the `ASMDB_MUX` base); later elements
//!   against the previous element. `ARCH_GROUP_DUMB::m_bits` likewise.
//! - Containers held by pointer (`m_block_lists` entries, index maps, the
//!   export/avalon maps) are preceded by a one-byte presence flag.

use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeMap;

use crate::pdb::{Cursor, Ptr};

/// Arch group types and block types are `ASMDB_ARCH_GROUP_TYPE` /
/// `ASMDB_ARCH_GROUP_BLOCK_TYPE` enum values. Their names come from the
/// translator table in libdb_asmdb.so; the handful the cram crate needs are
/// listed in `arch`.
pub mod arch {
    pub const LE_LUT_FUNCTION: i32 = 3;
    pub const LE_CLEAR_SELECT: i32 = 6;
    pub const LE_CLOCK_SELECT: i32 = 7;
    pub const LE_USE_SYNC_CLEAR: i32 = 9;
    pub const LE_USE_SYNC_LOAD: i32 = 10;
    pub const LE_USE_CARRY: i32 = 11;
    pub const LE_USE_REG_CASCADE_ARM: i32 = 15;
    pub const LE_USE_REGISTER_FEEDBACK: i32 = 18;
    pub const LE_REG_OUT_YGR: i32 = 19;
    pub const LE_LUT_INPUT: i32 = 21;
    pub const LAB_ENABLE_CE: i32 = 53;
    pub const LAB_ENABLE_SYNC_SIGNALS: i32 = 59;
    pub const LAB_PIA_TO_INPUT: i32 = 60;
    pub const LAB_CE_FAST_MUX: i32 = 61;
    pub const LAB_CLEAR_FAST_MUX: i32 = 62;
    pub const LAB_CLOCK_FAST_MUX: i32 = 63;
    pub const LAB_SYNC_CLR_FAST_MUX: i32 = 67;
    pub const LAB_INVERT: i32 = 69;
    pub const LAB_CLR_INVERT_PAIR: i32 = 70;
    pub const LAB_SYNC_LOAD_FAST_MUX_MAG: i32 = 74;
    pub const CLKBUF_CLK_INPUT_SELECT_CYCLONEII: i32 = 2318;
    pub const CLKBUF_ENABLE_AND_INVERT: i32 = 2320;
    pub const CLKBUF_ENABLE_MODE: i32 = 2339;
    pub const CLKBUF_INTERFACE_COREIN: i32 = 2333;
    /// Block type of a LAB (`m_group_block_type`).
    pub const BLOCK_LAB: i32 = 18;
    /// Block type of an M4K RAM block, and its groups.
    pub const BLOCK_M4K: i32 = 39;
    pub const RAM_MS_BYTE_ENABLE_INPUT: i32 = 570;
    pub const RAM_MS_READ_ADDRESS_INPUT: i32 = 572;
    pub const RAM_MS_WRITE_ADDRESS_INPUT: i32 = 573;
    pub const RAM_MS_ADDRESS_STALL_INPUT: i32 = 574;
    pub const RAM_MS_ADDRESS_STALL_INPUT_B: i32 = 575;
    pub const RAM_MS_DATA_INPUT: i32 = 576;
    pub const RAM_CE_FAST_MUX: i32 = 578;
    pub const RAM_CLEAR_FAST_MUX: i32 = 581;
    pub const RAM_CLOCK_FAST_MUX: i32 = 582;
    pub const RAM_RE_FAST_MUX: i32 = 583;
    pub const RAM_WE_FAST_MUX: i32 = 584;
    pub const RAM_CE_INVERT: i32 = 586;
    pub const RAM_CLEAR_INVERT: i32 = 587;
    pub const RAM_CLOCK_INVERT: i32 = 588;
    pub const RAM_READ_ENABLE_INVERT: i32 = 589;
    pub const RAM_WRITE_ENABLE_INVERT: i32 = 590;
    pub const RAM_DATA_WRITE_MODE: i32 = 591;
    pub const RAM_DATA_WRITE_MODE_B: i32 = 592;
    pub const RAM_REG_DATA_OUT: i32 = 593;
    pub const RAM_REG_DATA_OUT_B: i32 = 594;
    pub const RAM_SELECT_CLK_DATA_OUT: i32 = 599;
    pub const RAM_SELECT_CLK_DATA_OUT_B: i32 = 600;
    pub const RAM_SELECT_CLK_READ_ENABLE_READ_ADDRESS_B: i32 = 603;
    pub const RAM_SELECT_CLK_BYTE_ENABLE_DATA_B: i32 = 604;
    pub const RAM_ENABLE_CE_INPUT: i32 = 606;
    pub const RAM_ENABLE_CE_INPUT_B: i32 = 607;
    pub const RAM_ENABLE_CE_OUTPUT: i32 = 608;
    pub const RAM_ENABLE_CE_OUTPUT_B: i32 = 609;
    pub const RAM_SELECT_CLR_BYTE_ENABLE_B: i32 = 616;
    pub const RAM_SELECT_CLR_DATA_B: i32 = 617;
    pub const RAM_SELECT_CLR_DATA_OUT: i32 = 618;
    pub const RAM_SELECT_CLR_DATA_OUT_B: i32 = 619;
    pub const RAM_SELECT_CLR_READ_ADDRESS_B: i32 = 620;
    pub const RAM_SELECT_CLR_READ_ENABLE_B: i32 = 621;
    pub const RAM_USAGE_MODE: i32 = 624;
    pub const RAM_USE_CLR_DATA_OUT: i32 = 634;
    pub const RAM_USE_CLR_DATA_OUT_B: i32 = 635;
    pub const RAM_USE_EXTENDED_WRITE_PULSE_MAGELLAN: i32 = 642;
    pub const RAM_ENABLE: i32 = 643;
    pub const RAM_CONTENT_YGR_MEAB: i32 = 645;
    pub const RAM_USE_TRUE_DUAL_PORT_MODE: i32 = 648;
    pub const RAM_USE_PACKED_MODE: i32 = 649;
    pub const RAM_PIA_TO_INPUT_YGR: i32 = 650;
    pub const RAM_FLOW_THROUGH_MODE: i32 = 651;
    pub const RAM_FLOW_THROUGH_MODE_B: i32 = 652;
}

/// Index into `Asmdb::muxes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MuxId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MuxKind {
    Plain,
    Labeled { name: Option<String> },
    MappedSimple { map: Option<MuxId> },
    MappedEncoded { map: Option<MuxId> },
    StlMappedEncoded { index_map: Option<u32> },
    Decision { lower: bool, upper: bool },
    EncodedBank { mux_size: i32, bank_size: i32 },
    Ram { polarity: Option<MuxId> },
    Simple,
    Encoded,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Mux {
    /// Bit positions relative to the block base flat address (block base
    /// + bit = flat CRAM address), or for map/label muxes plain integers.
    pub bits: Vec<i32>,
    pub kind: MuxKind,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MuxData {
    pub arch_value: bool,
    pub aux: Option<MuxId>,
    pub next_selection: Option<MuxId>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ArchBody {
    /// `ARCH_GROUP_STD` (`block_info` none) and `ARCH_GROUP_CALC`.
    Muxed {
        paths: Vec<Vec<Option<MuxId>>>,
        data: Vec<MuxData>,
        block_info: Option<u32>,
        group_index_map: Option<MuxId>,
        offset_index_map: Option<MuxId>,
    },
    Dumb { arch_value: bool, bits: Vec<i32> },
    Stlmap { paths: Vec<Vec<Option<MuxId>>>, data: Vec<(bool, Option<u32>, Option<MuxId>, Option<u32>)> },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArchGroup {
    pub group_index: i32,
    pub arch_type: i32,
    pub block_type: i32,
    pub body: ArchBody,
}

impl ArchGroup {
    /// The mux paths of a muxed group: for LE-level groups one path per LE
    /// (16), each a list of muxes (levels of a two-level mux).
    pub fn paths(&self) -> &[Vec<Option<MuxId>>] {
        match &self.body {
            ArchBody::Muxed { paths, .. } | ArchBody::Stlmap { paths, .. } => paths,
            ArchBody::Dumb { .. } => &[],
        }
    }
    pub fn data(&self) -> &[MuxData] {
        match &self.body {
            ArchBody::Muxed { data, .. } => data,
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BlockInfo {
    pub block_type: i32,
    pub num_ram_cols: i32,
    /// Block instance base addresses, packed `row << 16 | column` in flat
    /// CRAM address space (flat = row * num_ram_cols + column).
    pub block_lists: BTreeMap<i32, Vec<u32>>,
    /// `(rows << 16) | columns` of one block instance.
    pub simple_block_lists: BTreeMap<i32, u32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Asmdb {
    pub num_bits: i32,
    pub block_info_map: Vec<(i32, u32)>,
    pub arch_groups: Vec<ArchGroup>,
    pub muxes: Vec<Mux>,
    pub block_infos: Vec<BlockInfo>,
    pub index_maps: Vec<Vec<(i32, i32)>>,
    pub default_bits: Vec<i32>,
    pub default_mux_settings: Vec<(Option<u32>, i32)>,
    pub default_bit_calcs: Vec<(bool, i32, Option<u32>)>,
}

impl Asmdb {
    pub fn mux(&self, id: MuxId) -> &Mux {
        &self.muxes[id.0 as usize]
    }
    pub fn block_info(&self, block_type: i32) -> Option<&BlockInfo> {
        self.block_info_map
            .iter()
            .find(|(t, _)| *t == block_type)
            .and_then(|(_, i)| self.block_infos.get(*i as usize))
    }
    /// Groups of one type for one block type, by `m_group_index`.
    pub fn groups(&self, block_type: i32, arch_type: i32) -> Vec<&ArchGroup> {
        let mut v: Vec<_> =
            self.arch_groups.iter().filter(|g| g.block_type == block_type && g.arch_type == arch_type).collect();
        v.sort_by_key(|g| g.group_index);
        v
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Table {
    Die,
    BlockInfo,
    Arch,
    Default,
    Mux,
    IndexMap,
}

fn table_of(class_id: u16) -> Result<Table> {
    Ok(match class_id {
        1 => Table::Die,
        2 => Table::BlockInfo,
        3 | 4 | 5 | 6 | 0x13 => Table::Arch,
        7 | 8 => Table::Default,
        9..=0xf | 0x11 | 0x12 | 0x14 => Table::Mux,
        0x10 => Table::IndexMap,
        c => bail!("asmdb: no pointer table for class {c:#x}"),
    })
}

/// Pointer bookkeeping: ids per table, the class of every pending object,
/// and the LIFO order tables get drained in.
#[derive(Default)]
struct Tables {
    /// table -> (class of each id, 1-based ids in order), last id resolved.
    classes: Vec<(Table, Vec<u16>, u32)>,
    stack: Vec<Table>,
    /// The table being drained; it is finished before anything else.
    current: Option<Table>,
}

impl Tables {
    fn slot(&mut self, t: Table) -> &mut (Table, Vec<u16>, u32) {
        if let Some(i) = self.classes.iter().position(|(x, _, _)| *x == t) {
            &mut self.classes[i]
        } else {
            self.classes.push((t, Vec::new(), 0));
            self.classes.last_mut().unwrap()
        }
    }
    /// Resolve a pointer read in a member whose targets live in `want`;
    /// returns the 1-based id.
    fn ptr(&mut self, p: Ptr, want: Table) -> Result<Option<u32>> {
        match p {
            Ptr::Null => Ok(None),
            Ptr::New { class_id } => {
                let t = table_of(class_id)?;
                ensure!(t == want, "asmdb: new {class_id:#x} in a {want:?} member");
                let s = self.slot(t);
                s.1.push(class_id);
                s.2 = s.1.len() as u32;
                let id = s.2;
                if !self.stack.contains(&t) && self.current != Some(t) {
                    self.stack.push(t);
                }
                Ok(Some(id))
            }
            Ptr::Ref { rel } => {
                let s = self.slot(want);
                let id = (s.2.wrapping_add(rel as u32)) & 0x3fff_ffff;
                ensure!(id >= 1 && id as usize <= s.1.len(), "asmdb: {want:?} back-reference to unknown id {id}");
                s.2 = id;
                Ok(Some(id))
            }
        }
    }
}

struct Reader<'a> {
    c: Cursor<'a>,
    t: Tables,
    prev_mux0: i32,
    prev_dumb0: i32,
    prev_group_index: i32,
    prev_setting: i32,
    prev_bit: i32,
    prev_ram_cols: i32,
    prev_mux_size: i32,
    prev_bank_size: i32,
    prev_index: (i32, i32),
}

impl<'a> Reader<'a> {
    fn ptr(&mut self, want: Table) -> Result<Option<u32>> {
        let p = self.c.ptr()?;
        self.t.ptr(p, want)
    }
    fn mux_ptr(&mut self) -> Result<Option<MuxId>> {
        Ok(self.ptr(Table::Mux)?.map(|i| MuxId(i - 1)))
    }
    fn count(&mut self) -> Result<usize> {
        let n = self.c.i32()?;
        ensure!(n >= 0, "asmdb: negative count {n} at {:#x}", self.c.pos() - 4);
        Ok(n as usize)
    }
    /// A vector of i32 deltas whose element 0 is a delta against `prev0`.
    fn delta_vec(&mut self, prev0: &mut i32) -> Result<Vec<i32>> {
        let n = self.count()?;
        let mut v = Vec::with_capacity(n);
        let mut acc = *prev0;
        for i in 0..n {
            acc = acc.wrapping_add(self.c.i32()?);
            if i == 0 {
                *prev0 = acc;
            }
            v.push(acc);
        }
        Ok(v)
    }
    fn empty_vec(&mut self, what: &str) -> Result<()> {
        let n = self.count()?;
        ensure!(n == 0, "asmdb: {what} has {n} entries; only empty seen on Cyclone II, not decoded");
        Ok(())
    }
    fn paths(&mut self) -> Result<Vec<Vec<Option<MuxId>>>> {
        let n = self.count()?;
        let mut paths = Vec::with_capacity(n);
        for _ in 0..n {
            let k = self.count()?;
            paths.push((0..k).map(|_| self.mux_ptr()).collect::<Result<_>>()?);
        }
        Ok(paths)
    }
    fn arch_base(&mut self) -> Result<(i32, i32, i32)> {
        self.prev_group_index = self.prev_group_index.wrapping_add(self.c.i32()?);
        Ok((self.prev_group_index, self.c.i32()?, self.c.i32()?))
    }

    fn mux(&mut self, class_id: u16) -> Result<Mux> {
        let mut p0 = self.prev_mux0;
        let bits = self.delta_vec(&mut p0)?;
        self.prev_mux0 = p0;
        let kind = match class_id {
            9 => MuxKind::Labeled { name: self.c.std_string_opt()? },
            0xa => MuxKind::MappedSimple { map: self.mux_ptr()? },
            0xb => MuxKind::Simple,
            0xc => MuxKind::Encoded,
            0xd => MuxKind::Decision { lower: self.c.bool()?, upper: self.c.bool()? },
            0xe => MuxKind::MappedEncoded { map: self.mux_ptr()? },
            0xf => MuxKind::StlMappedEncoded { index_map: self.ptr(Table::IndexMap)? },
            0x11 => {
                self.prev_mux_size = self.prev_mux_size.wrapping_add(self.c.i32()?);
                self.prev_bank_size = self.prev_bank_size.wrapping_add(self.c.i32()?);
                MuxKind::EncodedBank { mux_size: self.prev_mux_size, bank_size: self.prev_bank_size }
            }
            0x12 => MuxKind::Ram { polarity: self.mux_ptr()? },
            0x14 => MuxKind::Plain,
            c => bail!("asmdb: class {c:#x} is not a mux"),
        };
        Ok(Mux { bits, kind })
    }

    fn arch(&mut self, class_id: u16) -> Result<ArchGroup> {
        let (group_index, arch_type, block_type) = self.arch_base()?;
        let body = match class_id {
            3 | 4 => {
                let paths = self.paths()?;
                let n = self.count()?;
                let mut data = Vec::with_capacity(n);
                for _ in 0..n {
                    let arch_value = self.c.bool()?;
                    let aux = self.mux_ptr()?;
                    let next_selection = self.mux_ptr()?;
                    data.push(MuxData { arch_value, aux, next_selection });
                }
                let (block_info, group_index_map, offset_index_map) = if class_id == 3 {
                    (self.ptr(Table::BlockInfo)?, self.mux_ptr()?, self.mux_ptr()?)
                } else {
                    (None, None, None)
                };
                ArchBody::Muxed { paths, data, block_info, group_index_map, offset_index_map }
            }
            5 => {
                let arch_value = self.c.bool()?;
                let mut p0 = self.prev_dumb0;
                let bits = self.delta_vec(&mut p0)?;
                self.prev_dumb0 = p0;
                ArchBody::Dumb { arch_value, bits }
            }
            6 | 0x13 => {
                let paths = self.paths()?;
                let n = self.count()?;
                let mut data = Vec::with_capacity(n);
                for _ in 0..n {
                    let v = self.c.bool()?;
                    let aux_map = self.ptr(Table::IndexMap)?;
                    let next = self.mux_ptr()?;
                    let mask_map = self.ptr(Table::IndexMap)?;
                    data.push((v, aux_map, next, mask_map));
                }
                ArchBody::Stlmap { paths, data }
            }
            c => bail!("asmdb: class {c:#x} is not an arch group"),
        };
        Ok(ArchGroup { group_index, arch_type, block_type, body })
    }

    fn block_info(&mut self) -> Result<BlockInfo> {
        let _bcm_address_space_encoding = self.c.i32()?;
        let _version = self.c.std_string_opt()?;
        let _icd_name = self.c.std_string_opt()?;
        let mut block_lists = BTreeMap::new();
        for _ in 0..self.count()? {
            let key = self.c.i32()?;
            let mut v = Vec::new();
            if self.c.u8()? != 0 {
                // Each list starts from 0, unlike mux bit lists.
                v = self.delta_vec(&mut 0)?.into_iter().map(|x| x as u32).collect();
            }
            block_lists.insert(key, v);
        }
        let block_type = self.c.i32()?;
        self.prev_ram_cols = self.prev_ram_cols.wrapping_add(self.c.i32()?);
        let mut simple_block_lists = BTreeMap::new();
        for _ in 0..self.count()? {
            let key = self.c.i32()?;
            // Written as a delta, but against 0 every time (unlike the
            // block lists, which run on from the previous block info).
            simple_block_lists.insert(key, self.c.u32()?);
        }
        ensure!(self.c.u8()? == 0, "asmdb: export_port_to_bit_map present, not decoded");
        ensure!(self.c.u8()? == 0, "asmdb: block_bit_to_avalon_bit_map present, not decoded");
        self.empty_vec("avalon_name_to_size_map")?;
        Ok(BlockInfo { block_type, num_ram_cols: self.prev_ram_cols, block_lists, simple_block_lists })
    }

    fn index_map(&mut self) -> Result<Vec<(i32, i32)>> {
        let mut v = Vec::new();
        if self.c.u8()? != 0 {
            for _ in 0..self.count()? {
                self.prev_index.0 = self.prev_index.0.wrapping_add(self.c.i32()?);
                self.prev_index.1 = self.prev_index.1.wrapping_add(self.c.i32()?);
                v.push(self.prev_index);
            }
        }
        Ok(v)
    }
}

pub fn parse(body: &[u8]) -> Result<Asmdb> {
    let mut r = Reader {
        c: Cursor::new(body),
        t: Tables::default(),
        prev_mux0: 0,
        prev_dumb0: 0,
        prev_group_index: 0,
        prev_setting: 0,
        prev_bit: 0,
        prev_ram_cols: 0,
        prev_mux_size: 0,
        prev_bank_size: 0,
        prev_index: (0, 0),
    };
    ensure!(r.c.ptr()?.expect_new()? == 1, "asmdb body doesn't start with an ASMDB_DIE");
    let _arch_value = r.c.bool()?;
    let num_bits = r.c.i32()?;
    for _ in 0..3 {
        r.c.i32()?; // cff / unvm / option-register bit counts
    }
    for _ in 0..3 {
        r.c.std_string_opt()?; // version strings
    }
    let mut block_info_map = Vec::new();
    for _ in 0..r.count()? {
        let k = r.c.i32()?;
        let id = r.ptr(Table::BlockInfo)?.unwrap_or(0);
        block_info_map.push((k, id - 1));
    }
    for _ in 0..r.count()? {
        r.ptr(Table::Arch)?;
    }
    for _ in 0..r.count()? {
        r.ptr(Table::Default)?;
    }
    let mut p0 = 0;
    let default_bits = r.delta_vec(&mut p0)?;
    r.empty_vec("forced_bit_list")?;
    for _ in 0..r.count()? {
        r.ptr(Table::Mux)?;
    }
    for what in ["local_default_group_list", "local_arch_group_list", "hc_base_nodes", "hc_fringe_nodes", "hc_wires"] {
        r.empty_vec(what)?;
    }

    let mut db = Asmdb {
        num_bits,
        block_info_map,
        arch_groups: Vec::new(),
        muxes: Vec::new(),
        block_infos: Vec::new(),
        index_maps: Vec::new(),
        default_bits,
        default_mux_settings: Vec::new(),
        default_bit_calcs: Vec::new(),
    };
    // Drain pointer tables LIFO, each completely and in id order, until
    // nothing is pending.
    loop {
        let t = match r.t.current {
            Some(t) => t,
            None => match r.t.stack.pop() {
                Some(t) => {
                    r.t.current = Some(t);
                    t
                }
                None => break,
            },
        };
        let done = match t {
            Table::Arch => db.arch_groups.len(),
            Table::Mux => db.muxes.len(),
            Table::BlockInfo => db.block_infos.len(),
            Table::IndexMap => db.index_maps.len(),
            Table::Default => db.default_mux_settings.len() + db.default_bit_calcs.len(),
            Table::Die => bail!("asmdb: a second ASMDB_DIE"),
        };
        let class = r.t.slot(t).1.get(done).copied();
        let Some(class) = class else {
            r.t.current = None;
            continue;
        };
        match t {
            Table::Arch => {
                let g = r.arch(class).with_context(|| format!("arch group #{done} class {class:#x}"))?;
                db.arch_groups.push(g);
            }
            Table::Mux => {
                let m = r.mux(class).with_context(|| format!("mux #{done} class {class:#x}"))?;
                db.muxes.push(m);
            }
            Table::BlockInfo => {
                let b = r.block_info()?;
                db.block_infos.push(b);
            }
            Table::IndexMap => {
                let m = r.index_map()?;
                db.index_maps.push(m);
            }
            Table::Default => {
                // Both default classes share a table; keep ids aligned by
                // pushing to both lists' combined count.
                if class == 7 {
                    let g = r.ptr(Table::Arch)?;
                    r.prev_setting = r.prev_setting.wrapping_add(r.c.i32()?);
                    db.default_mux_settings.push((g, r.prev_setting));
                } else {
                    let v = r.c.bool()?;
                    r.prev_bit = r.prev_bit.wrapping_add(r.c.i32()?);
                    let bi = r.ptr(Table::BlockInfo)?;
                    db.default_bit_calcs.push((v, r.prev_bit, bi));
                }
            }
            Table::Die => unreachable!(),
        }
    }
    ensure!(r.c.remaining() <= 8, "asmdb: {} trailing bytes", r.c.remaining());
    Ok(db)
}
