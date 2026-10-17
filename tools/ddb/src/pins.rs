//! Package and pad databases: `ddb_<die><package>_pkg.ddb`
//! (`DEV_PKG_INFO_BODY`, one `DEV_PIN` per package ball) and
//! `ddb_<die>_pad.ddb` (`DEV_PAD_INFO_BODY`, one `DEV_PAD` per die pad).
//! A pin names its pads by `DEV_PAD_BODY::m_id`; a pad carries its IOE
//! location as a string like "X0Y34SUB_LOC0".
//!
//! Both bodies use the generic "info body" schema: fixed arrays
//! (`m_int_data[N]`, `m_string_data[N]`, ...) indexed by per-class enums
//! whose names are not in the file, plus `*_enum_is_valid` bit vectors that
//! say which slots are populated. The serialization layout here is checked
//! token for token against the `PDB_ASCII_DUMP` trace (see
//! `verify/pins_trace_tokens.py`); which slot means what is mostly inferred,
//! and each field says which.

use anyhow::{bail, ensure, Context, Result};
use std::collections::HashMap;
use std::path::Path;

use crate::container::Ddb;
use crate::pdb::{Cursor, Ptr};

/// One die pad (`DEV_PAD_BODY`).
#[derive(Debug, Clone)]
pub struct Pad {
    /// `m_id`, the pad id that `DEV_PIN::m_pad_id_list` refers to. **[obs]**
    /// It equals the "Pad Number" column of the fit report's "All Package
    /// Pins" table. **[obs]**
    pub id: i32,
    /// IOE tile x, y and sub-location z, parsed from `m_string_data[1]`
    /// ("X0Y34SUB_LOC0"). Matches the fit report's X/Y/cell. **[obs]**
    /// `None` for pads with no location string (none on magellan4).
    pub loc: Option<(u16, u16, u8)>,
    /// `m_int_data[0]`: a global id; `m_ids_by_global_id` maps it back to
    /// `id`. **[obs]** for the mapping, **[inf]** for what the id names.
    pub global_id: i32,
    /// `m_int_data[2] + 1`: I/O bank, 1..8. **[obs]**: equals the fit
    /// report's "I/O Bank" for all 487 pads.
    pub bank: i32,
    /// `m_int_data[12]`: VREF group, 0..15, two per bank. **[inf]**
    pub vref_group: i32,
    /// `m_int_data[11]`: id of the VREF pad of this pad's group; 0 on the
    /// VREF pads themselves (and pad 0 is a real pad, so 0 is ambiguous).
    /// **[inf]**: every nonzero value is a pad whose name is "VREFB<n>N<m>".
    pub vref_pad: i32,
    /// `m_int_data[6]`: id of the other pad of a differential pair (2<->3,
    /// 4<->5, ...), 0 if none. **[inf]**
    pub diff_partner: i32,
    /// `m_string_data[3]`: "Row I/O", "Column I/O", "Dedicated Clock",
    /// "Dedicated Programming", "JTAG". **[obs]**: equals the fit report's
    /// "I/O Type", except that the report files the 16 dedicated clock
    /// inputs as Row/Column I/O by the edge they sit on.
    pub io_type: Option<String>,
    /// `m_string_data[4]`: pin function as the pin-out lists it
    /// ("LVDS151n", "CLK4, LVDSCLK2p, Input", "VREFB2N0", "ASDO"). **[inf]**
    pub function: Option<String>,
    /// `m_string_data[5]`: pad name, unique per pad: "P<n><L|R|T|B><k>"
    /// for user I/O ("P1L0", "P0B0"), the function for dedicated pins
    /// ("CLK0", "TDI"). **[inf]**
    pub pad_name: Option<String>,
    /// `m_string_data[6]`: second name ("SDO", "LVDSB0A", "VREF0B1",
    /// "CLK4"). **[inf]**
    pub alt_name: Option<String>,
    /// Names of the I/O standards this pad supports (`m_io_standards`,
    /// resolved through `DEV_IO_STANDARD_DESC_BODY::m_string_data[0]`).
    /// **[obs]** for the pointer resolution, **[inf]** for the meaning.
    pub io_standards: Vec<String>,
    pub raw: InfoBody,
}

/// One package ball (`DEV_PIN`).
#[derive(Debug, Clone)]
pub struct Pin {
    /// `m_id`, sequential from 0. **[obs]**
    pub id: i32,
    /// `m_string_data[0]`: ball name, "E4", "AE23". **[obs]**
    pub name: String,
    /// `m_string_data[1]`: power/config function for balls without a user
    /// pad ("VCCD_PLL3", "GND_PLL3"), else empty. **[inf]**
    pub function: Option<String>,
    /// `m_pad_id_list`: `Pad::id`s bonded to this ball. **[obs]**
    pub pad_ids: Vec<i32>,
    pub raw: InfoBody,
}

/// The generic per-object arrays, kept so callers can look at slots that
/// are not named yet.
#[derive(Debug, Clone, Default)]
pub struct InfoBody {
    pub bools: Vec<bool>,
    pub ints: Vec<i32>,
    pub strings: Vec<Option<String>>,
    pub vec_ints: Vec<Vec<i32>>,
}

pub struct PadDb {
    pub pads: Vec<Pad>,
    /// `DEV_IO_STANDARD_DESC_BODY::m_string_data[0]` of every I/O standard,
    /// in table order ("LVTTL", "SSTL_2_CLASS_I", ...). **[obs]**
    pub io_standards: Vec<String>,
}

pub struct PkgDb {
    pub pins: Vec<Pin>,
}

/// A pin joined with its pads, as `ddb pins` prints it.
pub struct PinMap<'a> {
    pub pin: &'a Pin,
    pub pads: Vec<&'a Pad>,
}

/// Open `ddb_<die>_pad.ddb` and `ddb_<die><package>_pkg.ddb` from a
/// devinfo family directory and join them.
pub fn load(devinfo: &Path, die: &str, package: &str) -> Result<(PkgDb, PadDb)> {
    let open = |name: String| -> Result<Ddb> {
        let p = devinfo.join(name);
        Ddb::open(&p).with_context(|| format!("opening {}", p.display()))
    };
    let pad = parse_pad(&open(format!("ddb_{die}_pad.ddb"))?, None)?;
    let pkg = parse_pkg(&open(format!("ddb_{die}{package}_pkg.ddb"))?, None)?;
    Ok((pkg, pad))
}

pub fn join<'a>(pkg: &'a PkgDb, pad: &'a PadDb) -> Result<Vec<PinMap<'a>>> {
    let by_id: HashMap<i32, &Pad> = pad.pads.iter().map(|p| (p.id, p)).collect();
    pkg.pins
        .iter()
        .map(|pin| {
            let pads = pin
                .pad_ids
                .iter()
                .map(|id| by_id.get(id).copied().with_context(|| format!("pin {} names unknown pad {id}", pin.name)))
                .collect::<Result<_>>()?;
            Ok(PinMap { pin, pads })
        })
        .collect()
}

/// "X0Y34SUB_LOC0" -> (0, 34, 0).
pub fn parse_loc(s: &str) -> Option<(u16, u16, u8)> {
    let s = s.strip_prefix('X')?;
    let (x, s) = s.split_once('Y')?;
    let (y, z) = s.split_once("SUB_LOC")?;
    Some((x.parse().ok()?, y.parse().ok()?, z.parse().ok()?))
}

// ---------------------------------------------------------------------------
// Serialization. Delta rules seen in the trace: a scalar member references
// the same member of the previous object of its class (zero for the first);
// in a fixed array or vector, element 0 does that and element i references
// element i-1; map keys and values chain from 0.

/// Previous-object values per member, for one class.
#[derive(Default)]
struct Refs(HashMap<&'static str, i32>);

/// Per-class pointer table (`read_ptr`): new objects take sequential ids
/// from 1, back-references are relative to the last id resolved.
#[derive(Default, Clone, Copy)]
struct Table {
    count: u32,
    last: u32,
}

struct Rd<'a> {
    c: Cursor<'a>,
    /// The value stream in `verify/pins_trace_tokens.py` form.
    log: Option<&'a mut Vec<String>>,
    tables: HashMap<u16, Table>,
}

impl<'a> Rd<'a> {
    fn emit(&mut self, s: impl FnOnce() -> String) {
        if let Some(l) = self.log.as_deref_mut() {
            l.push(s());
        }
    }
    fn raw(&mut self) -> Result<i32> {
        let v = self.c.i32()?;
        self.emit(|| format!("{v:08X}"));
        Ok(v)
    }
    fn count(&mut self) -> Result<usize> {
        let n = self.raw()?;
        ensure!((0..1 << 24).contains(&n), "bad count {n} at {:#x}", self.c.pos() - 4);
        Ok(n as usize)
    }
    fn delta(&mut self, base: i32) -> Result<i32> {
        let v = base.wrapping_add(self.c.i32()?);
        self.emit(|| format!("{v:08X}"));
        Ok(v)
    }
    /// A delta-encoded scalar member.
    fn int(&mut self, r: &mut Refs, key: &'static str) -> Result<i32> {
        let v = self.delta(*r.0.get(key).unwrap_or(&0))?;
        r.0.insert(key, v);
        Ok(v)
    }
    /// `n` chained elements; element 0 references `r[key]` (or 0 if `key`
    /// is None).
    fn chain(&mut self, r: &mut Refs, key: Option<&'static str>, n: usize) -> Result<Vec<i32>> {
        let mut out = Vec::with_capacity(n);
        let mut prev = key.and_then(|k| r.0.get(k).copied()).unwrap_or(0);
        for _ in 0..n {
            prev = self.delta(prev)?;
            out.push(prev);
        }
        if let (Some(k), Some(&v0)) = (key, out.first()) {
            r.0.insert(k, v0);
        }
        Ok(out)
    }
    /// `std::vector<bool>`: bit count, then the 32-bit words, chained.
    fn bits(&mut self, r: &mut Refs, key: &'static str) -> Result<Vec<bool>> {
        let n = self.count()?;
        let words = self.chain(r, Some(key), n.div_ceil(32))?;
        Ok((0..n).map(|i| words[i / 32] >> (i % 32) & 1 != 0).collect())
    }
    /// i32 length then exactly that many bytes; 0 is an empty/null string.
    /// The trace prints a NUL, but none is stored.
    fn string(&mut self) -> Result<Option<String>> {
        let n = self.count()?;
        if n == 0 {
            return Ok(None);
        }
        let s = String::from_utf8_lossy(self.c.bytes(n)?).into_owned();
        self.emit(|| format!("\"{s}\""));
        Ok(Some(s))
    }
    fn strings(&mut self, n: usize) -> Result<Vec<Option<String>>> {
        (0..n).map(|_| self.string()).collect()
    }
    fn f64(&mut self) -> Result<f64> {
        let v = f64::from_le_bytes(self.c.bytes(8)?.try_into().unwrap());
        // C's %E: two-digit signed exponent.
        self.emit(|| {
            let s = format!("{v:.6E}");
            let (m, e) = s.split_once('E').unwrap();
            let e: i32 = e.parse().unwrap();
            format!("{m}E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
        });
        Ok(v)
    }
    /// A pointer into `class`'s table; returns the object id (None = null).
    fn ptr(&mut self, class: u16) -> Result<Option<u32>> {
        let raw = self.raw()?;
        let t = self.tables.entry(class).or_default();
        let id = match Ptr::decode(raw) {
            Ptr::Null => return Ok(None),
            Ptr::New { class_id } => {
                ensure!(class_id == class, "pointer to class {class_id}, expected {class}");
                t.count += 1;
                t.count
            }
            Ptr::Ref { rel } => (t.last.wrapping_add(rel as u32)) & 0x3fff_ffff,
        };
        ensure!(id >= 1 && id <= t.count, "pointer id {id} outside table of {} (class {class})", t.count);
        t.last = id;
        Ok(Some(id))
    }
    fn ptrs(&mut self, class: u16) -> Result<Vec<Option<u32>>> {
        let n = self.count()?;
        (0..n).map(|_| self.ptr(class)).collect()
    }
    fn begin(&mut self, class: &str) {
        self.emit(|| format!("@{class}"));
    }
    /// A member whose element format has not been seen: every instance in
    /// these files is empty, so insist on that.
    fn empty(&mut self, what: &str) -> Result<()> {
        let n = self.raw()?;
        ensure!(n == 0, "{what}: non-empty ({n}) and its element format is unknown, at {:#x}", self.c.pos() - 4);
        Ok(())
    }
    /// `vector<vector<int>>`: row count, then per row a count and chained ints.
    fn int_2d(&mut self) -> Result<Vec<Vec<i32>>> {
        let rows = self.count()?;
        let mut none = Refs::default();
        (0..rows)
            .map(|_| {
                let n = self.count()?;
                self.chain(&mut none, None, n)
            })
            .collect()
    }
}

// Footer class ids differ per file; the parsers look them up by name.
fn class_ids(footer: &[u8]) -> Result<HashMap<String, u16>> {
    Ok(crate::pdb::parse_class_table(footer)?.into_iter().map(|c| (c.name, c.class_id as u16)).collect())
}

fn cid(ids: &HashMap<String, u16>, name: &str) -> Result<u16> {
    ids.get(name).copied().with_context(|| format!("class {name} missing from footer"))
}

pub fn parse_pad(d: &Ddb, log: Option<&mut Vec<String>>) -> Result<PadDb> {
    parse_pad_with(&d.body, &class_ids(&d.footer)?, log)
}

pub fn parse_pkg(d: &Ddb, log: Option<&mut Vec<String>>) -> Result<PkgDb> {
    parse_pkg_with(&d.body, &class_ids(&d.footer)?, log)
}

fn parse_pad_with(body: &[u8], ids: &HashMap<String, u16>, log: Option<&mut Vec<String>>) -> Result<PadDb> {
    let (c_info, c_pad, c_body, c_std, c_std_body) = (
        cid(ids, "DEV_PAD_INFO_BODY")?,
        // The family-wide cycloneii_pad has no pads and lacks both classes.
        cid(ids, "DEV_PAD").unwrap_or(0),
        cid(ids, "DEV_PAD_BODY").unwrap_or(0),
        cid(ids, "DEV_IO_STANDARD_DESC")?,
        cid(ids, "DEV_IO_STANDARD_DESC_BODY")?,
    );
    let mut r = Rd { c: Cursor::new(body), log, tables: HashMap::new() };
    ensure!(r.ptr(c_info)? == Some(1), "top object is not a new DEV_PAD_INFO_BODY");
    r.begin("DEV_PAD_INFO_BODY");

    // DEV_PAD_INFO_BODY
    let mut ri = Refs::default();
    let pad_list = r.ptrs(c_pad)?;
    let default_body = r.ptr(c_body)?;
    r.bits(&mut ri, "bool")?;
    r.chain(&mut ri, Some("int"), 27)?;
    r.strings(10)?;
    for _ in 0..5 {
        let n = r.count()?;
        r.strings(n)?;
    }
    for i in 0..4 {
        r.empty(&format!("DEV_PAD_INFO_BODY::m_vec_int_data[{i}]"))?;
    }
    for _ in 0..13 {
        r.int_2d()?;
    }
    for k in ["v_bool", "v_int", "v_string", "v_vec_string", "v_vec_int", "v_2d_int"] {
        r.bits(&mut ri, k)?;
    }
    // m_ids_by_global_id: map<int, int>
    let n = r.count()?;
    let (mut k, mut v) = (0, 0);
    for _ in 0..n {
        k = r.delta(k)?;
        v = r.delta(v)?;
    }
    // m_ids_by_string: map<string, int>
    let n = r.count()?;
    let mut v = 0;
    for _ in 0..n {
        r.string()?;
        v = r.delta(v)?;
    }
    let n_std = r.ptrs(c_std)?.len();
    // m_io_standards_by_string: map<string, DEV_IO_STANDARD_DESC*>
    let n = r.count()?;
    for _ in 0..n {
        r.string()?;
        r.ptr(c_std)?;
    }
    for _ in 0..105 {
        r.ptr(c_std)?; // m_default_io_standards_by_enum
    }
    // m_io_standard_enums: each element is read twice in the trace, as the
    // int and as the "enum" it converts to.
    let n = r.count()?;
    for _ in 0..n {
        let v = r.raw()?;
        r.emit(|| format!("{v:08X}"));
    }
    ensure!(r.raw()? == 0, "m_io_rules is not null");

    // Deferred bodies, in the order the trace shows.
    let mut rs = Refs::default();
    for _ in 0..n_std {
        r.begin("DEV_IO_STANDARD_DESC");
        r.ptr(c_std_body)?.context("null DEV_IO_STANDARD_DESC::m_body")?;
    }
    let mut io_standards = Vec::with_capacity(n_std);
    for _ in 0..n_std {
        r.begin("DEV_IO_STANDARD_DESC_BODY");
        r.bits(&mut rs, "bool")?;
        r.chain(&mut rs, Some("int"), 32)?;
        let s = r.strings(27)?;
        io_standards.push(s[0].clone().unwrap_or_default());
        for _ in 0..19 {
            r.int_2d()?;
        }
        let n = r.count()?; // m_current_set, same int + enum pairs
        for _ in 0..n {
            let v = r.raw()?;
            r.emit(|| format!("{v:08X}"));
        }
        r.chain(&mut rs, Some("gnd"), 18)?;
        r.chain(&mut rs, Some("vcc"), 18)?;
        for k in ["v_bool", "v_int", "v_string", "v_current", "v_2d_int"] {
            r.bits(&mut rs, k)?;
        }
    }

    let mut rb = Refs::default();
    let mut pad_body = |r: &mut Rd| -> Result<(i32, InfoBody, Vec<Option<u32>>)> {
        r.begin("DEV_PAD_BODY");
        let id = r.int(&mut rb, "id")?;
        r.ptr(c_body)?; // m_default_pad_body
        let bools = r.bits(&mut rb, "bool")?;
        let ints = r.chain(&mut rb, Some("int"), 53)?;
        let strings = r.strings(20)?;
        for i in 0..10 {
            r.empty(&format!("DEV_PAD_BODY::m_2d_int_data[{i}]"))?;
        }
        for i in 0..4 {
            r.empty(&format!("DEV_PAD_BODY::m_string_hash_data[{i}]"))?;
        }
        for k in ["v_bool", "v_int", "v_string", "v_2d_int", "v_string_hash"] {
            r.bits(&mut rb, k)?;
        }
        let stds = r.ptrs(c_std)?;
        for _ in 0..105 {
            r.ptr(c_std)?; // m_io_standards_by_enum
        }
        Ok((id, InfoBody { bools, ints, strings, vec_ints: vec![] }, stds))
    };
    if default_body.is_some() {
        pad_body(&mut r)?;
    }
    for _ in &pad_list {
        r.begin("DEV_PAD");
        r.ptr(c_body)?.context("null DEV_PAD::m_body")?;
    }
    let mut pads = Vec::with_capacity(pad_list.len());
    for _ in &pad_list {
        let (id, raw, stds) = pad_body(&mut r)?;
        let s = |i: usize| raw.strings[i].clone();
        pads.push(Pad {
            id,
            loc: s(1).as_deref().map(|l| parse_loc(l).with_context(|| format!("pad {id}: bad location {l:?}"))).transpose()?,
            global_id: raw.ints[0],
            bank: raw.ints[2] + 1,
            vref_group: raw.ints[12],
            vref_pad: raw.ints[11],
            diff_partner: raw.ints[6],
            io_type: s(3),
            function: s(4),
            pad_name: s(5),
            alt_name: s(6),
            io_standards: stds
                .iter()
                .map(|p| {
                    let i = p.context("null io standard")? as usize;
                    io_standards.get(i - 1).cloned().context("io standard id out of range")
                })
                .collect::<Result<_>>()?,
            raw,
        });
    }
    ensure!(r.c.is_empty(), "{} trailing bytes in pad body", r.c.remaining());
    Ok(PadDb { pads, io_standards })
}

fn parse_pkg_with(body: &[u8], ids: &HashMap<String, u16>, log: Option<&mut Vec<String>>) -> Result<PkgDb> {
    let (c_info, c_pin) = (cid(ids, "DEV_PKG_INFO_BODY")?, cid(ids, "DEV_PIN")?);
    let mut r = Rd { c: Cursor::new(body), log, tables: HashMap::new() };
    ensure!(r.ptr(c_info)? == Some(1), "top object is not a new DEV_PKG_INFO_BODY");
    r.begin("DEV_PKG_INFO_BODY");

    let mut ri = Refs::default();
    let pin_list = r.ptrs(c_pin)?;
    r.bits(&mut ri, "bool")?;
    r.chain(&mut ri, Some("int"), 19)?;
    r.strings(10)?;
    for _ in 0..11 {
        r.int_2d()?;
    }
    for _ in 0..5 {
        let n = r.count()?; // m_vec_double_data
        for _ in 0..n {
            r.f64()?;
        }
    }
    for i in 0..4 {
        r.empty(&format!("DEV_PKG_INFO_BODY::m_vec_int_data[{i}]"))?;
    }
    for k in ["v_bool", "v_int", "v_string", "v_2d_int", "v_vec_double", "v_vec_int"] {
        r.bits(&mut ri, k)?;
    }
    for k in ["p_bool", "p_int", "p_string", "p_vec_int", "p_string_hash"] {
        r.bits(&mut ri, k)?;
    }
    let n = r.count()?; // m_ids_by_string
    let mut v = 0;
    for _ in 0..n {
        r.string()?;
        v = r.delta(v)?;
    }

    let mut rp = Refs::default();
    let mut pins = Vec::with_capacity(pin_list.len());
    for _ in &pin_list {
        r.begin("DEV_PIN");
        let id = r.int(&mut rp, "id")?;
        let bools = r.bits(&mut rp, "bool")?;
        let ints = r.chain(&mut rp, Some("int"), 24)?;
        let strings = r.strings(12)?;
        let mut vec_ints = Vec::with_capacity(7);
        for _ in 0..7 {
            let n = r.count()?;
            vec_ints.push(r.chain(&mut rp, None, n)?);
        }
        for i in 0..4 {
            r.empty(&format!("DEV_PIN::m_string_hash_data[{i}]"))?;
        }
        for k in ["v_bool", "v_int", "v_string", "v_vec_int", "v_string_hash"] {
            r.bits(&mut rp, k)?;
        }
        let n = r.count()?;
        // Element 0 references 0 here, not the previous pin's list. [obs]
        let pad_ids = r.chain(&mut rp, None, n)?;
        let name = strings[0].clone().with_context(|| format!("pin {id} has no name"))?;
        let function = strings[1].clone();
        pins.push(Pin { id, name, function, pad_ids, raw: InfoBody { bools, ints, strings, vec_ints } });
    }
    ensure!(r.c.is_empty(), "{} trailing bytes in pkg body", r.c.remaining());
    if pins.is_empty() {
        bail!("no pins");
    }
    Ok(PkgDb { pins })
}
