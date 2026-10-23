//! The device database this repo ships (`db/EP2C35F672C6/`), so that
//! packing, the base image and nextpnr's chip database need no Quartus
//! files at use time. `cram db` derives it from Quartus' devinfo once; every
//! use-time command reads only it.
//!
//! Two files, both xz:
//!
//! - `device.bin.xz`: `MAGIC`, `VERSION` (u32, little-endian), then the
//!   postcard encoding (<https://postcard.jamesmunns.com/wire-format>) of
//!   `Stored`: the routing graph, every routing element's CRAM bits, the
//!   asmdb's block tables and the package's user pins. The schema is the
//!   Rust types it names (`Stored` down, including `ddb::asmdb::Asmdb` and
//!   `io::Pin`); bump `VERSION` whenever one of them changes.
//! - `chipdb.txt.xz`: `cram chipdb` output, the input of
//!   `nextpnr/cyclone2/gen/arch_gen.py` (nextpnr builds with Python only).
//!   `cram db` exports it from the `device.bin.xz` it just wrote, read back,
//!   so the two can't drift apart.

use anyhow::{bail, ensure, Context, Result};
use std::io::{BufReader, BufWriter, Read, Write};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use ddb::asm::{AsmBit, AsmNode};
use ddb::asmdb::Asmdb;
use ddb::device::Device;
use ddb::routing::{Element, Routing, Template};
use lzma_rust2::{XzOptions, XzReader, XzWriterMt};

use crate::io::{IoDb, Pin};
use crate::lab::LabDb;
use crate::phys::Db;

/// The one device there is a database for.
pub const DEVICE: &str = "EP2C35F672C6";
pub const DEVICE_FILE: &str = "device.bin.xz";
pub const CHIPDB_FILE: &str = "chipdb.txt.xz";

const MAGIC: &[u8; 8] = b"C2RE-DB\0";
const VERSION: u32 = 1;

/// What `device.bin.xz` holds. Everything else in `Db` is derived from it on
/// load, which is cheap next to decompressing it.
#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    routing: WireRouting,
    asm: WireAsm,
    asmdb: Asmdb,
    pins: Vec<Pin>,
}

/// The routing graph by column. Neighbouring elements sit in neighbouring
/// tiles and fan out to neighbouring ids, so each column is stored as
/// differences from the entry before it (`deltas`): small varints that xz
/// squeezes far better than the absolute values.
#[derive(serde::Serialize, serde::Deserialize)]
struct WireRouting {
    x: Vec<i32>,
    y: Vec<i32>,
    fanout_edge_base: Vec<i32>,
    template: Vec<i32>,
    templates: Vec<WireTemplate>,
}

/// `ddb::routing::Template` with the edge count implied by the fanout list,
/// and each edge's target offset and position kept together.
#[derive(serde::Serialize, serde::Deserialize)]
struct WireTemplate {
    element_enum: i16,
    z: i16,
    index: i16,
    first_purely_redundant_edge: i16,
    length: i8,
    metal_layer: i8,
    direction: i8,
    /// (fanout offset, fanout-to position), both as deltas.
    fanout: Vec<(i32, i32)>,
}

/// Every element's CRAM bits (`ddb::asm::AsmNode`) by column: per element
/// its group count and bit count, then all group starts and all bits.
#[derive(serde::Serialize, serde::Deserialize)]
struct WireAsm {
    groups: Vec<u32>,
    bits: Vec<u32>,
    group_starts: Vec<u16>,
    /// Flat CRAM addresses, as deltas through all elements' bits.
    flat_address: Vec<i32>,
    kind: Vec<BitKind>,
}

/// The only flag combinations the asm file has (0 or 0xff each); anything
/// else would be a new kind of bit the packer doesn't know how to set.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
enum BitKind {
    /// On when its edge is used.
    Plain,
    /// Part of an encoded mux setting: its edge wants it low / high.
    EncodedLow,
    EncodedHigh,
}

impl BitKind {
    fn of(b: &AsmBit) -> Result<Self> {
        Ok(match (b.use_encoded_setting, b.is_encoded_bit_high, b.is_strangely_encoded, b.is_cff_bit) {
            (0, 0, 0, 0) => BitKind::Plain,
            (0xff, 0, 0, 0) => BitKind::EncodedLow,
            (0xff, 0xff, 0, 0) => BitKind::EncodedHigh,
            f => bail!("routing bit {} has flags {f:?}, which the database can't store", b.flat_address),
        })
    }
    fn bit(self, flat_address: u32) -> AsmBit {
        let (enc, high) = match self {
            BitKind::Plain => (0, 0),
            BitKind::EncodedLow => (0xff, 0),
            BitKind::EncodedHigh => (0xff, 0xff),
        };
        AsmBit { flat_address, use_encoded_setting: enc, is_encoded_bit_high: high, is_strangely_encoded: 0, is_cff_bit: 0 }
    }
}

/// Each value minus the one before it (wrapping, so any u32 round-trips).
fn deltas(v: impl IntoIterator<Item = u32>) -> Vec<i32> {
    let mut prev = 0u32;
    v.into_iter()
        .map(|x| {
            let d = x.wrapping_sub(prev) as i32;
            prev = x;
            d
        })
        .collect()
}

fn undeltas(d: &[i32]) -> impl Iterator<Item = u32> + '_ {
    d.iter().scan(0u32, |prev, &x| {
        *prev = prev.wrapping_add(x as u32);
        Some(*prev)
    })
}

impl WireRouting {
    fn new(r: &Routing) -> Self {
        let e = &r.elements;
        WireRouting {
            x: deltas(e.iter().map(|e| e.x as u32)),
            y: deltas(e.iter().map(|e| e.y as u32)),
            fanout_edge_base: deltas(e.iter().map(|e| e.fanout_edge_base)),
            template: deltas(e.iter().map(|e| e.template)),
            templates: r
                .templates
                .iter()
                .map(|t| WireTemplate {
                    element_enum: t.element_enum,
                    z: t.z,
                    index: t.index,
                    first_purely_redundant_edge: t.first_purely_redundant_edge,
                    length: t.length,
                    metal_layer: t.metal_layer,
                    direction: t.direction,
                    fanout: deltas(t.fanout_offsets.iter().map(|&o| o as u32))
                        .into_iter()
                        .zip(deltas(t.fanout_to_positions.iter().map(|&p| p as u32)))
                        .collect(),
                })
                .collect(),
        }
    }

    fn routing(self) -> Result<Routing> {
        let n = self.x.len();
        ensure!([self.y.len(), self.fanout_edge_base.len(), self.template.len()].iter().all(|&l| l == n), "routing columns differ in length");
        let elements: Vec<Element> = undeltas(&self.x)
            .zip(undeltas(&self.y))
            .zip(undeltas(&self.fanout_edge_base))
            .zip(undeltas(&self.template))
            .map(|(((x, y), fanout_edge_base), template)| Element { x: x as i16, y: y as i16, fanout_edge_base, template })
            .collect();
        ensure!(elements.iter().all(|e| (e.template as usize) < self.templates.len()), "element names a missing template");
        let templates = self
            .templates
            .into_iter()
            .map(|t| {
                let (offsets, positions): (Vec<i32>, Vec<i32>) = t.fanout.into_iter().unzip();
                Template {
                    element_enum: t.element_enum,
                    num_edges: offsets.len() as i16,
                    z: t.z,
                    index: t.index,
                    first_purely_redundant_edge: t.first_purely_redundant_edge,
                    length: t.length,
                    metal_layer: t.metal_layer,
                    direction: t.direction,
                    fanout_offsets: undeltas(&offsets).map(|o| o as i32).collect(),
                    fanout_to_positions: undeltas(&positions).map(|p| p as i16).collect(),
                }
            })
            .collect();
        Ok(Routing { elements, templates })
    }
}

impl WireAsm {
    fn new(asm: &[AsmNode]) -> Result<Self> {
        let all_bits = || asm.iter().flat_map(|a| &a.bits);
        Ok(WireAsm {
            groups: asm.iter().map(|a| a.start_bit_index.len() as u32).collect(),
            bits: asm.iter().map(|a| a.bits.len() as u32).collect(),
            group_starts: asm.iter().flat_map(|a| a.start_bit_index.iter().copied()).collect(),
            flat_address: deltas(all_bits().map(|b| b.flat_address)),
            kind: all_bits().map(BitKind::of).collect::<Result<_>>()?,
        })
    }

    fn nodes(self) -> Result<Vec<AsmNode>> {
        ensure!(self.groups.len() == self.bits.len(), "asm columns differ in length");
        ensure!(self.group_starts.len() as u64 == self.groups.iter().map(|&g| g as u64).sum::<u64>(), "asm group count mismatch");
        ensure!(self.flat_address.len() == self.kind.len(), "asm bit columns differ in length");
        ensure!(self.kind.len() as u64 == self.bits.iter().map(|&b| b as u64).sum::<u64>(), "asm bit count mismatch");
        let mut starts = self.group_starts.into_iter();
        let mut bits = undeltas(&self.flat_address).zip(self.kind).map(|(a, k)| k.bit(a));
        Ok(self
            .groups
            .iter()
            .zip(&self.bits)
            .map(|(&g, &b)| AsmNode {
                start_bit_index: starts.by_ref().take(g as usize).collect(),
                bits: bits.by_ref().take(b as usize).collect(),
            })
            .collect())
    }
}

impl Db {
    /// Straight from Quartus' device files (`cram db` and the RE commands).
    pub fn from_quartus(devinfo: &Path) -> Result<Db> {
        let asmdb = ddb::asmdb::parse(&ddb::container::Ddb::open(&devinfo.join("ddb_magellan4_asmdb.ddb"))?.body)?;
        let pins = crate::io::pins_from_quartus(devinfo)?;
        Self::new(Device::load(devinfo, "magellan4")?, asmdb, &pins)
    }

    fn new(dev: Device, asmdb: Asmdb, pins: &[Pin]) -> Result<Db> {
        Ok(Db { lab: LabDb::new(&asmdb)?, io: IoDb::new(pins, &asmdb)?, m4k: crate::m4k::M4kDb::new(&asmdb)?, dev, asmdb })
    }

    /// The shipped database in `dir` (a device directory such as
    /// `db/EP2C35F672C6`).
    pub fn load(dir: &Path) -> Result<Db> {
        let p = dir.join(DEVICE_FILE);
        let f = std::fs::File::open(&p).with_context(|| format!("opening {}", p.display()))?;
        let mut raw = Vec::new();
        XzReader::new(BufReader::new(f), false).read_to_end(&mut raw).with_context(|| format!("decompressing {}", p.display()))?;
        let body = raw.strip_prefix(MAGIC).with_context(|| format!("{} is not a cram device database", p.display()))?;
        let (version, body) = body.split_first_chunk::<4>().context("truncated header")?;
        let version = u32::from_le_bytes(*version);
        ensure!(version == VERSION, "{}: format version {version}, this cram reads {VERSION} (rerun `just db`, or update cram)", p.display());
        let s: Stored = postcard::from_bytes(body).with_context(|| format!("decoding {}", p.display()))?;
        Self::new(Device::new(s.routing.routing()?, s.asm.nodes()?)?, s.asmdb, &s.pins)
    }

    /// The `device.bin.xz` encoding of this database, uncompressed.
    fn encode(&self) -> Result<Vec<u8>> {
        let mut pins: Vec<Pin> = self.io.pins.values().cloned().collect();
        pins.sort_by(|a, b| a.name.cmp(&b.name));
        let s = Stored {
            routing: WireRouting::new(&self.dev.routing),
            asm: WireAsm::new(&self.dev.asm)?,
            asmdb: self.asmdb.clone(),
            pins,
        };
        let mut out = MAGIC.to_vec();
        out.extend(VERSION.to_le_bytes());
        postcard::to_extend(&s, out).context("encoding the device database")
    }
}

/// Write `<root>/EP2C35F672C6/` from Quartus' device files, then read it
/// back: the reloaded database must encode to the same bytes, and the chip
/// database text comes from it rather than from Quartus' files.
pub fn build(devinfo: &Path, root: &Path) -> Result<()> {
    let dir = root.join(DEVICE);
    std::fs::create_dir_all(&dir)?;
    let encoded = Db::from_quartus(devinfo)?.encode()?;
    write_xz(&dir.join(DEVICE_FILE), &encoded)?;
    let db = Db::load(&dir)?;
    ensure!(db.encode()? == encoded, "the device database does not read back as written");
    let mut text = Vec::new();
    crate::chipdb::export(&db, &mut text)?;
    write_xz(&dir.join(CHIPDB_FILE), &text)
}

/// The output must be a function of the input alone (it is committed):
/// fixed-size blocks, each compressed independently, come out the same
/// whatever the thread count. Preset 6's 8 MiB dictionary, so the 124 MB
/// chip database text compresses in parallel.
fn write_xz(path: &Path, data: &[u8]) -> Result<()> {
    let mut opts = XzOptions::with_preset(6);
    opts.set_block_size(NonZeroU64::new(BLOCK_SIZE));
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
    let tmp = path.with_extension("xz.tmp");
    let mut w = XzWriterMt::new(BufWriter::new(std::fs::File::create(&tmp)?), opts, threads)?;
    w.write_all(data)?;
    w.finish()?.flush()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

const BLOCK_SIZE: u64 = 16 << 20;

/// Where use-time commands find the database: `$CRAM_DB/EP2C35F672C6` if
/// set; else the `db/` of the checkout this binary was built in, found by
/// walking up from the executable (`tools/cram/target/<profile>/cram`) and,
/// failing that, at the path cargo built it from.
pub fn locate() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os("CRAM_DB") {
        return Ok(PathBuf::from(root).join(DEVICE));
    }
    let has_db = |root: &Path| root.join("db").join(DEVICE).join(DEVICE_FILE).is_file();
    let exe = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok());
    let built_in = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    match exe.iter().flat_map(|e| e.ancestors()).find(|a| has_db(a)) {
        Some(root) => Ok(root.join("db").join(DEVICE)),
        None if has_db(&built_in) => Ok(built_in.join("db").join(DEVICE)),
        None => bail!("no device database (db/{DEVICE}/{DEVICE_FILE}) above the cram executable or in its source tree; set CRAM_DB to the db/ directory"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deltas_round_trip() {
        let v = [0, 5, 3, u32::MAX, 0x8000_0000, 7];
        assert_eq!(undeltas(&deltas(v)).collect::<Vec<_>>(), v);
    }

    #[test]
    fn routing_and_asm_round_trip() {
        let t = |e, offs: Vec<i32>| Template {
            element_enum: e,
            num_edges: offs.len() as i16,
            z: 2,
            index: -1,
            first_purely_redundant_edge: offs.len() as i16,
            length: 4,
            metal_layer: 1,
            direction: -1,
            fanout_to_positions: (0..offs.len() as i16).rev().collect(),
            fanout_offsets: offs,
        };
        let r = Routing {
            elements: vec![
                Element { x: 3, y: 35, fanout_edge_base: 0x8000_0010, template: 1 },
                Element { x: -1, y: 0, fanout_edge_base: 0x8000_0000, template: 0 },
            ],
            templates: vec![t(264, vec![1, -1, 70_000]), t(43, vec![])],
        };
        let back = WireRouting::new(&r).routing().unwrap();
        assert_eq!(format!("{:?}", back.elements), format!("{:?}", r.elements));
        assert_eq!(format!("{:?}", back.templates), format!("{:?}", r.templates));

        let asm = vec![
            AsmNode { start_bit_index: vec![1, 0, 3], bits: vec![BitKind::Plain.bit(9), BitKind::EncodedHigh.bit(2), BitKind::EncodedLow.bit(6_707_826)] },
            AsmNode::default(),
        ];
        let back = WireAsm::new(&asm).unwrap().nodes().unwrap();
        assert_eq!(format!("{back:?}"), format!("{asm:?}"));

        let odd = AsmBit { is_cff_bit: 1, ..BitKind::Plain.bit(1) };
        assert!(WireAsm::new(&[AsmNode { start_bit_index: vec![1], bits: vec![odd] }]).is_err());
    }
}
