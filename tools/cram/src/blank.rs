//! The base image of an unconfigured-design RBF, built without Quartus:
//! container framing, the asmdb's default bits, and the values unused
//! blocks hold. `cram pack` writes a design's bits over it.
//!
//! The per-feature values below were read off Quartus' empty design (one
//! output pin tied low; README "Blank image") by attributing every set bit
//! to the asmdb feature that owns it (tools/cram/examples/blank_probe.rs).
//! They are checked by assembling all 179 fuzz designs on this base instead
//! of Quartus' empty RBF (`cram roundtrip - ...`).

use anyhow::{ensure, Context, Result};

use ddb::asmdb::Asmdb;

use crate::io::{IoBit, IoDb, UnusedPins};
use crate::rbf::{self, Rbf};
use crate::{addr, le};

/// Which paths / block instances an entry covers.
#[derive(Clone, Copy)]
enum Sel {
    All,
    Only(usize),
}

impl Sel {
    fn has(self, i: usize) -> bool {
        match self {
            Sel::All => true,
            Sel::Only(k) => k == i,
        }
    }
}

use Sel::{All, Only};

/// Bits that are 1 in every unused block: (block type, arch group type,
/// group index, path, block instance, mux level, bit within the mux).
/// Block types: 18 LAB, 39 M4K, 43 PLL, 75 global clock control.
#[rustfmt::skip]
const UNUSED_ONES: &[(i32, i32, i32, Sel, Sel, usize, &[usize])] = &[
    // LE_CLOCK_SELECT 1 and LE_USE_REG_CASCADE_ARM on every LE.
    (18, 7, -1, All, All, 0, &[0]),
    (18, 15, -1, All, All, 0, &[0]),
    // No clock enables, no sync signals (code 101), clear fast muxes and
    // the sync clear mux at their idle setting, LAB_INVERT 1, 3, 4.
    (18, 53, 0, All, All, 0, &[0]),
    (18, 53, 1, All, All, 0, &[0]),
    (18, 59, -1, All, All, 0, &[0, 2]),
    (18, 62, 0, All, All, 0, &[1]),
    (18, 62, 1, All, All, 0, &[1]),
    (18, 67, 0, All, All, 0, &[0]),
    (18, 69, 1, All, All, 0, &[0]),
    (18, 69, 3, All, All, 0, &[0]),
    (18, 69, 4, All, All, 0, &[0]),
    // M4K blocks.
    (39, 578, 0, All, All, 0, &[0]),
    (39, 578, 1, All, All, 0, &[0]),
    (39, 581, 0, All, All, 0, &[0]),
    (39, 581, 1, All, All, 0, &[0]),
    (39, 582, 0, All, All, 0, &[1]),
    (39, 582, 1, All, All, 0, &[1]),
    (39, 583, -1, All, All, 0, &[1]),
    (39, 584, -1, All, All, 0, &[1]),
    (39, 599, -1, All, All, 0, &[0]),
    (39, 600, -1, All, All, 0, &[0]),
    (39, 603, -1, All, All, 0, &[0]),
    (39, 604, -1, All, All, 0, &[0]),
    (39, 624, -1, All, All, 0, &[1, 2, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15, 16, 25]),
    // PLL 2 (only that one; the others are all 0) and one bit pair of PLL 3.
    (43, 745, -1, Only(0), Only(2), 0, &[6, 8]),
    (43, 745, -1, Only(1), Only(2), 0, &[6, 8]),
    (43, 745, -1, Only(2), Only(2), 0, &[9, 11]),
    (43, 745, -1, Only(3), Only(2), 0, &[9, 11]),
    (43, 748, -1, Only(0), Only(2), 0, &[7, 9]),
    (43, 748, -1, Only(1), Only(2), 0, &[7, 9]),
    (43, 750, -1, Only(2), Only(2), 0, &[8, 10]),
    (43, 750, -1, Only(3), Only(2), 0, &[8, 10]),
    (43, 778, 0, Only(0), Only(3), 0, &[0, 1]),
    (43, 778, 1, Only(0), Only(3), 0, &[3]),
    (43, 2335, 3, Only(0), Only(2), 0, &[8, 10]),
    // Global clock controls: input select 111, CLKBUF_ENABLE_AND_INVERT.
    (75, 2318, -1, All, All, 0, &[0, 1, 2]),
    (75, 2320, -1, All, All, 0, &[0]),
];

/// One set bit per LAB that no asmdb feature names: frame `lut_frame + 2`,
/// LAB row 36. The centre clock column (39) has one per LAB row in frame
/// 1663. Never seen changing.
const UNNAMED_LAB_DF: u32 = 2;
const UNNAMED_LAB_ROW: u32 = 36;
const UNNAMED_CENTRE_FRAME: u32 = 1663;

/// RBF bytes 0x4c..0x75: bit 2 of bytes 0x4d..0x6d is the USERCODE, LSB
/// first; bits 0-1 are this pattern, the same in every RBF with a pinned
/// USERCODE (190 of them). What they encode is unknown.
const ID_BLOCK_LOW_BITS: [u8; 41] = [
    0, 0, 1, 0, 0, 0, 1, 1, 0, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 2, 2, 3, 2, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 2, 3,
    3, 2, 3,
];

/// The container: preamble, option words, USERCODE block, frame area
/// (zero), postamble. Layout in README "Container format".
fn container(usercode: u32) -> Vec<u8> {
    let mut b = vec![0u8; rbf::TOTAL_BYTES];
    b[..0x10].fill(0xff);
    for k in 0..3 {
        b[0x10 + 9 * k..0x19 + 9 * k].copy_from_slice(&[0x6a, 0xdf, 0xff, 0x7f, 0x00, 0x60, 0x80, 0x68, 0x00]);
    }
    b[0x2c..0x4c].fill(0xff);
    for (i, &low) in ID_BLOCK_LOW_BITS.iter().enumerate() {
        let k = i as i64 - 1;
        let uc = (0..32).contains(&k) && usercode >> k & 1 == 1;
        b[0x4c + i] = low | (u8::from(uc) << 2);
    }
    b[0x77..rbf::HEADER_BYTES].fill(0x01);
    let tail = rbf::TOTAL_BYTES - rbf::TAIL_BYTES;
    b[tail..].fill(0xff);
    b
}

pub fn image(db: &Asmdb, io: &IoDb) -> Result<Rbf> {
    let mut r = Rbf::parse(container(0xffff_ffff))?;
    let set = |r: &mut Rbf, flat: u32| r.set(addr::frame_bit(flat), true);
    for &b in &db.default_bits {
        set(&mut r, b as u32);
    }
    for &(block, arch, group, paths, insts, level, bits) in UNUSED_ONES {
        let g = db
            .arch_groups
            .iter()
            .find(|g| (g.block_type, g.arch_type, g.group_index) == (block, arch, group))
            .with_context(|| format!("asmdb has no group {block}/{arch}/{group}"))?;
        let bases: Vec<u32> = match db.block_info(block) {
            Some(bi) => bi.block_lists.values().flatten().map(|&p| (p >> 16) * bi.num_ram_cols as u32 + (p & 0xffff)).collect(),
            None => vec![0],
        };
        for (pi, p) in g.paths().iter().enumerate().filter(|(i, _)| paths.has(*i)) {
            let m = p.get(level).copied().flatten().context("missing mux level")?;
            for (_, &base) in bases.iter().enumerate().filter(|(k, _)| insts.has(*k)) {
                for &bit in bits {
                    let off = db.mux(m).bits[bit];
                    ensure!(off >= 0, "group {block}/{arch}/{group} path {pi}: no bit {bit}");
                    set(&mut r, (base as i64 + off as i64) as u32);
                }
            }
        }
    }
    let lab = crate::lab::LabDb::new(db)?;
    for (x, y) in lab.lab_locs() {
        let frame = le::lut_frame(x).context("LAB column")? + UNNAMED_LAB_DF;
        r.set(addr::frame_row(frame, le::lab_row(y) + UNNAMED_LAB_ROW), true);
    }
    for y in 1..=35 {
        r.set(addr::frame_row(UNNAMED_CENTRE_FRAME, le::lab_row(y) + UNNAMED_LAB_ROW), true);
    }
    for (b, v) in io.bits(&[], UnusedPins::InputTriStated)? {
        match b {
            IoBit::Core(b) => r.set(b, v),
            IoBit::Rep(b) => r.rep_set(b, v),
        }
    }
    r.fix_crcs();
    Ok(r)
}
