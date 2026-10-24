//! M4K RAM blocks: what a placed design configures in one, in physical
//! terms, and the CRAM bits that follow.
//!
//! Device side (asmdb block type 39, observed): 105 blocks, one per LAB row
//! in the M4K columns X13, X26 and X52. A block is 70 CRAM rows x 166
//! columns, based at `le::lab_row(y)` and the block's highest frame
//! (`le::lut_frame(x - 1) + 12 + 166`); mux bits are flat offsets from that
//! base, like a LAB's. Every feature below is an asmdb group of the block;
//! what fuzzing (README "M4K RAM blocks") supplied is which logical port pin
//! each physical input is, the memory content order, and the setting values.
//!
//! The memory is 128 rows x 36 columns; `RAM_CONTENT_YGR_MEAB` lists its
//! 4,608 bits column-major (bit `col * 128 + row`), stored as written (not
//! inverted). A port of physical width P (1, 2, 4, 9, 18, 36 bits) sees it
//! as 4608/P words (4096/P for P < 9: those widths skip every ninth column)
//! laid out row-major: word `a` bit `s` is linear bit `a*P + s` of the 36-
//! column rows.

use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeMap, HashMap};

use ddb::asmdb::{arch, Asmdb};

use crate::lab::Feature;
use crate::rcf::Signal;
use crate::vo::Atom;
use crate::{addr, le};

pub const ROWS: usize = 128;
pub const COLS: usize = 36;
pub const BITS: usize = ROWS * COLS;
/// LAB-grid columns that hold M4K blocks.
pub const COLUMNS: [u32; 3] = [13, 26, 52];

/// Physical width of a port: how it addresses the 128 x 36 array. A port
/// configured x8, x16 or x32 is physically x9, x18, x36 with the ninth bits
/// unused (or used for data, when the fitter permutes bits into them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Width {
    X1,
    X2,
    X4,
    X9,
    X18,
    X36,
}

impl Width {
    pub fn bits(self) -> usize {
        [1, 2, 4, 9, 18, 36][self.class()]
    }
    /// Address bits: 12 for x1 down to 7 for x36.
    pub fn addr_bits(self) -> u32 {
        12 - self.class() as u32
    }
    /// Index into the one-hot `RAM_DATA_WRITE_MODE` (fuzz: x1 bit 0 .. x36
    /// bit 5).
    fn class(self) -> usize {
        self as usize
    }
    pub fn from_addr_bits(n: u32) -> Option<Self> {
        Some(match n {
            12 => Width::X1,
            11 => Width::X2,
            10 => Width::X4,
            9 => Width::X9,
            8 => Width::X18,
            7 => Width::X36,
            _ => return None,
        })
    }
    /// The column of this port's word that linear position `l` falls in.
    pub fn column(self, l: usize) -> usize {
        let p = self.bits();
        if p < 9 {
            (l / 9 * 8 + l % 9) % p
        } else {
            l % p
        }
    }
    /// Linear position in the 36-column rows of bit `slot` of word `a`.
    pub fn linear(self, a: usize, slot: usize) -> usize {
        let p = self.bits();
        if p < 9 {
            let l = a * p + slot;
            l / 8 * 9 + l % 8
        } else {
            a * p + slot
        }
    }
}

/// The RAM contents in `RAM_CONTENT_YGR_MEAB` order (`col * 128 + row`).
#[derive(Clone, PartialEq, Eq)]
pub struct Content(Box<[u64; BITS / 64]>);

impl Default for Content {
    fn default() -> Self {
        Content(Box::new([0; BITS / 64]))
    }
}

impl std::fmt::Debug for Content {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ones: u32 = self.0.iter().map(|w| w.count_ones()).sum();
        write!(f, "Content({ones} of {BITS} set)")
    }
}

impl Content {
    /// Set the bit at linear position `l` of the 36-column rows.
    pub fn set_linear(&mut self, l: usize) {
        let (row, col) = (l / COLS, l % COLS);
        let i = col * ROWS + row;
        self.0[i / 64] |= 1 << (i % 64);
    }
    /// The bit at linear position `l` of the 36-column rows.
    pub fn get_linear(&self, l: usize) -> bool {
        self.get(l % COLS * ROWS + l / COLS)
    }
    pub fn get(&self, i: usize) -> bool {
        self.0[i / 64] >> (i % 64) & 1 == 1
    }
}

/// One of the block's two clocks / clears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Sel {
    Zero,
    One,
}

/// A port that reads: its width and its optional output register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadPort {
    pub width: Width,
    pub out_reg: Option<OutReg>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutReg {
    pub clock: Sel,
    pub clear: Option<Sel>,
}

/// How the block's ports are used. Port A is always clocked by clock 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Port A reads and writes (a ROM is this without a write enable).
    Single { a: ReadPort },
    /// Port A writes, port B reads; port B's address register on `read_clock`.
    /// Without `write`: a x36 ROM, which Quartus builds as a read-only port B.
    SimpleDual { write: Option<Width>, read: ReadPort, read_clock: Sel },
    /// Both ports read and write, port B's input registers on `b_clock`.
    /// Widths up to x18 (`Mode::true_dual` checks).
    TrueDual { a: ReadPort, b: ReadPort, b_clock: Sel },
    /// Two x18 single ports on one clock, port A on words 128-255 and port
    /// B on 0-127 of a 256 x 18 block: how Quartus builds a 128 x 32/36
    /// single port.
    Packed { a_out: Option<OutReg>, b_out: Option<OutReg> },
}

impl Mode {
    pub fn true_dual(a: ReadPort, b: ReadPort, b_clock: Sel) -> Result<Mode> {
        ensure!(a.width <= Width::X18 && b.width <= Width::X18, "true dual port wider than x18");
        Ok(Mode::TrueDual { a, b, b_clock })
    }
}

/// A control signal reaching one of the block's control fast muxes: one of
/// the 8 control inputs (`RAM_PIA_TO_INPUT_YGR`, fed from a LAB line) or a
/// row clock (`BLK_CLK_BUF` 0-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtlSrc {
    Control(u8),
    Global(u8),
}

impl CtlSrc {
    /// Source index in the fast muxes' asmdb maps: 0-7 control inputs,
    /// 8-13 row clocks (observed on the clock, clear and write-enable muxes).
    fn index(self) -> usize {
        match self {
            CtlSrc::Control(k) => k as usize,
            CtlSrc::Global(g) => 8 + g as usize,
        }
    }
}

/// The block's physical input pins, named by the asmdb group that selects
/// their LAB line. `DATA_INPUT` 18-35 are the same muxes as `DATA_INPUT_B`
/// 0-17, `BYTE_ENABLE_INPUT` 2-3 as `BYTE_ENABLE_INPUT_B` 0-1 (identical bit
/// lists), so those groups don't appear here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Input {
    /// 0-35: data in, port A's columns 0-17 (36 for x36), port B's 18-35.
    Data(u8),
    /// 0-6 and 11.
    WriteAddr(u8),
    /// 0-6, 9, 10, 11.
    ReadAddr(u8),
    ByteEna(u8),
}

impl Input {
    /// Every physical input pin, in chip database order.
    pub fn all() -> Vec<Input> {
        let mut v: Vec<Input> = (0..36).map(Input::Data).collect();
        v.extend((0..7).chain([11]).map(Input::WriteAddr));
        v.extend((0..7).chain([9, 10, 11]).map(Input::ReadAddr));
        v.extend((0..4).map(Input::ByteEna));
        v
    }

    /// Bel pin name in the chip database (`chipdb.rs`), which the nextpnr
    /// uarch's packer targets.
    pub fn pin_name(self) -> String {
        match self {
            Input::Data(k) => format!("DI{k}"),
            Input::WriteAddr(k) => format!("WA{k}"),
            Input::ReadAddr(k) => format!("RA{k}"),
            Input::ByteEna(k) => format!("BE{k}"),
        }
    }

    pub fn from_pin_name(s: &str) -> Option<Input> {
        let num = |p: &str| s.strip_prefix(p)?.parse::<u8>().ok();
        let i = if let Some(k) = num("DI") {
            Input::Data(k)
        } else if let Some(k) = num("WA") {
            Input::WriteAddr(k)
        } else if let Some(k) = num("RA") {
            Input::ReadAddr(k)
        } else {
            Input::ByteEna(num("BE")?)
        };
        Input::all().contains(&i).then_some(i)
    }

    /// The asmdb source mux (arch type, group) that picks its LAB line.
    pub fn group(self) -> (i32, i32) {
        match self {
            Input::Data(k) => (arch::RAM_MS_DATA_INPUT, k as i32),
            Input::WriteAddr(k) => (arch::RAM_MS_WRITE_ADDRESS_INPUT, k as i32),
            Input::ReadAddr(k) => (arch::RAM_MS_READ_ADDRESS_INPUT, k as i32),
            Input::ByteEna(k) => (arch::RAM_MS_BYTE_ENABLE_INPUT, k as i32),
        }
    }

    /// Physical address pin `p` (0-11, LSB first; a port of width w uses
    /// pins 12 - addr_bits(w) ..= 11) of port A / B. Fuzz: port A's low pins
    /// are data inputs 12-15 (unused below x18), port B's are data inputs
    /// 26 and 35 and read address 9-10.
    pub fn addr(port_b: bool, p: u32) -> Result<Input> {
        ensure!(p < 12, "address pin {p}");
        Ok(match (port_b, p) {
            (false, 0..=3) => Input::Data(12 + p as u8),
            (false, 4) => Input::WriteAddr(11),
            (false, _) => Input::WriteAddr(p as u8 - 5),
            (true, 0) => Input::Data(26),
            (true, 1) => Input::Data(35),
            (true, 2..=4) => Input::ReadAddr(7 + p as u8),
            (true, _) => Input::ReadAddr(p as u8 - 5),
        })
    }
}

/// The block's control fast muxes and the LAB lines feeding its 8 control
/// inputs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Controls {
    /// LAB line driving control input k (`RAM_PIA_TO_INPUT_YGR` k).
    pub pia: [Option<u8>; 8],
    pub clock: [Option<CtlSrc>; 2],
    pub enable: [Option<CtlSrc>; 2],
    /// Clock enable k is active low (`RAM_CE_INVERT` k). Only meaningful
    /// with `enable[k]`.
    pub enable_inverted: [bool; 2],
    pub clear: [Option<CtlSrc>; 2],
    /// Port A write enable.
    pub write_enable: Option<CtlSrc>,
    /// Port B write enable (`portbrewe`; true dual port only).
    pub b_write_enable: Option<CtlSrc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct M4k {
    pub x: u32,
    pub y: u32,
    pub mode: Mode,
    pub controls: Controls,
    /// Which LAB line feeds each used input pin.
    pub inputs: BTreeMap<Input, u8>,
    pub content: Content,
}

/// The asmdb's M4K tables.
pub struct M4kDb {
    features: HashMap<(i32, i32), Feature>,
    content: Vec<i32>,
    bases: HashMap<(u32, u32), u32>,
}

impl M4kDb {
    pub fn new(db: &Asmdb) -> Result<Self> {
        let mut features = HashMap::new();
        for g in db.arch_groups.iter().filter(|g| g.block_type == arch::BLOCK_M4K) {
            features.insert((g.arch_type, g.group_index), Feature::from(db, g));
        }
        let content = features.get(&(arch::RAM_CONTENT_YGR_MEAB, -1)).context("asmdb has no M4K content group")?.paths[0][0].clone();
        ensure!(content.len() == BITS, "M4K content has {} bits", content.len());
        let bi = db.block_info(arch::BLOCK_M4K).context("asmdb has no M4K block info")?;
        let cols = bi.num_ram_cols as u32;
        ensure!(cols == addr::COLUMNS, "asmdb row width {cols} != {}", addr::COLUMNS);
        let mut bases = HashMap::new();
        for &packed in bi.block_lists.values().flatten() {
            let (row, col) = (packed >> 16, packed & 0xffff);
            let y = 35 - (row - le::lab_row(35)) / le::LAB_ROWS;
            ensure!(le::lab_row(y) == row, "asmdb M4K at row {row} is not a LAB row start");
            let x = *COLUMNS
                .iter()
                .find(|&&x| block_column(x) == Some(col))
                .with_context(|| format!("asmdb M4K at column {col} is in no M4K column"))?;
            bases.insert((x, y), row * cols + col);
        }
        ensure!(bases.len() == 105, "{} M4K blocks", bases.len());
        Ok(M4kDb { features, content, bases })
    }

    pub fn feature(&self, t: i32, i: i32) -> Result<&Feature> {
        self.features.get(&(t, i)).with_context(|| format!("asmdb M4K group {t}/{i} missing"))
    }

    fn flat(&self, x: u32, y: u32, off: i32) -> Result<u32> {
        let b = self.bases.get(&(x, y)).with_context(|| format!("no M4K at X{x} Y{y}"))?;
        Ok((*b as i64 + off as i64) as u32)
    }

    /// Base flat address of block (x, y); feature offsets count from it.
    pub fn base(&self, x: u32, y: u32) -> Result<u32> {
        self.flat(x, y, 0)
    }

    /// (x, y) of every block.
    pub fn locs(&self) -> Vec<(u32, u32)> {
        let mut v: Vec<_> = self.bases.keys().copied().collect();
        v.sort();
        v
    }
}

/// Flat column of an M4K block's base: the last frame of the 166 between
/// LAB columns x-1 and x+1 (observed: asmdb columns 2083, 1493, 456).
fn block_column(x: u32) -> Option<u32> {
    Some(addr::LAST_FRAME - (le::lut_frame(x - 1)? + 12 + 166))
}

impl M4k {
    /// Every bit this block fixes, as (flat CRAM address, value). Bits not
    /// listed keep the unused-block value of the base image.
    pub fn bits(&self, db: &M4kDb) -> Result<Vec<(u32, bool)>> {
        let mut out = Vec::new();
        let (x, y) = (self.x, self.y);
        let mut put = |off: i32, v: bool| -> Result<()> {
            out.push((db.flat(x, y, off)?, v));
            Ok(())
        };
        let one = |t: i32| -> Result<i32> { Ok(db.feature(t, -1)?.bit(0)) };
        // RAM_ENABLE in every used block; the extended write pulse in all
        // but a true dual port whose port A never writes (fuzz: d8k; a ROM
        // keeps it).
        let a_reads_only = matches!(self.mode, Mode::TrueDual { .. }) && self.controls.write_enable.is_none();
        for (t, on) in [(arch::RAM_USE_EXTENDED_WRITE_PULSE_MAGELLAN, !a_reads_only), (arch::RAM_ENABLE, true)] {
            for &b in &db.feature(t, -1)?.paths[0][0] {
                put(b, on)?;
            }
        }
        for t in [arch::RAM_ENABLE_CE_INPUT, arch::RAM_ENABLE_CE_INPUT_B, arch::RAM_ENABLE_CE_OUTPUT, arch::RAM_ENABLE_CE_OUTPUT_B] {
            put(one(t)?, true)?;
        }
        let write_mode = |t: i32, w: Width| -> Result<Vec<(i32, bool)>> {
            Ok(db.feature(t, -1)?.paths[0][0].iter().enumerate().map(|(i, &b)| (b, i == w.class())).collect())
        };
        let (a_out, b_out) = match self.mode {
            Mode::Single { a } => {
                // Single port is the true-dual-port block, with read-during-
                // write flow-through on both ports when it writes (fuzz; a
                // ROM has none).
                put(one(arch::RAM_USE_TRUE_DUAL_PORT_MODE)?, true)?;
                let writes = self.controls.write_enable.is_some();
                for t in [arch::RAM_FLOW_THROUGH_MODE, arch::RAM_FLOW_THROUGH_MODE_B] {
                    put(one(t)?, writes)?;
                }
                for (b, v) in write_mode(arch::RAM_DATA_WRITE_MODE, a.width)? {
                    put(b, v)?;
                }
                (a.out_reg, None)
            }
            Mode::SimpleDual { write, read, read_clock } => {
                for (b, v) in write.map(|w| write_mode(arch::RAM_DATA_WRITE_MODE, w)).transpose()?.unwrap_or_default() {
                    put(b, v)?;
                }
                for (b, v) in write_mode(arch::RAM_DATA_WRITE_MODE_B, read.width)? {
                    put(b, v)?;
                }
                put(one(arch::RAM_SELECT_CLK_READ_ENABLE_READ_ADDRESS_B)?, read_clock == Sel::One)?;
                // x36 writes use port B's data-in registers for columns
                // 18-35, which then take port A's clock 0 (fuzz).
                if write == Some(Width::X36) {
                    put(one(arch::RAM_SELECT_CLK_BYTE_ENABLE_DATA_B)?, false)?;
                }
                // The read data leaves through the port-A output register
                // group (fuzz: a registered port B clears REG_DATA_OUT, not
                // REG_DATA_OUT_B). A x36 read uses both groups, columns 18-35
                // leaving through port B's (x6).
                (read.out_reg, if read.width == Width::X36 { read.out_reg } else { None })
            }
            Mode::TrueDual { a, b, b_clock } => {
                for t in [arch::RAM_USE_TRUE_DUAL_PORT_MODE, arch::RAM_FLOW_THROUGH_MODE, arch::RAM_FLOW_THROUGH_MODE_B] {
                    put(one(t)?, true)?;
                }
                for (bit, v) in write_mode(arch::RAM_DATA_WRITE_MODE, a.width)? {
                    put(bit, v)?;
                }
                for (bit, v) in write_mode(arch::RAM_DATA_WRITE_MODE_B, b.width)? {
                    put(bit, v)?;
                }
                put(one(arch::RAM_SELECT_CLK_READ_ENABLE_READ_ADDRESS_B)?, b_clock == Sel::One)?;
                put(one(arch::RAM_SELECT_CLK_BYTE_ENABLE_DATA_B)?, b_clock == Sel::One)?;
                (a.out_reg, b.out_reg)
            }
            Mode::Packed { a_out, b_out } => {
                // A true dual port, both x18 on clock 0, plus the packed
                // flag (s32, s36).
                for t in [arch::RAM_USE_TRUE_DUAL_PORT_MODE, arch::RAM_USE_PACKED_MODE, arch::RAM_FLOW_THROUGH_MODE, arch::RAM_FLOW_THROUGH_MODE_B] {
                    put(one(t)?, true)?;
                }
                for t in [arch::RAM_DATA_WRITE_MODE, arch::RAM_DATA_WRITE_MODE_B] {
                    for (bit, v) in write_mode(t, Width::X18)? {
                        put(bit, v)?;
                    }
                }
                put(one(arch::RAM_SELECT_CLK_READ_ENABLE_READ_ADDRESS_B)?, false)?;
                put(one(arch::RAM_SELECT_CLK_BYTE_ENABLE_DATA_B)?, false)?;
                (a_out, b_out)
            }
        };
        let regs = [
            (a_out, arch::RAM_REG_DATA_OUT, arch::RAM_SELECT_CLK_DATA_OUT, arch::RAM_USE_CLR_DATA_OUT, arch::RAM_SELECT_CLR_DATA_OUT),
            (b_out, arch::RAM_REG_DATA_OUT_B, arch::RAM_SELECT_CLK_DATA_OUT_B, arch::RAM_USE_CLR_DATA_OUT_B, arch::RAM_SELECT_CLR_DATA_OUT_B),
        ];
        for (reg, t_reg, t_clk, t_use_clr, t_clr) in regs {
            // REG_DATA_OUT reads 1 for an unregistered output (asmdb map
            // [1, 0]); the clock select's unused value is 1.
            put(one(t_reg)?, reg.is_none())?;
            if let Some(r) = reg {
                put(one(t_clk)?, r.clock == Sel::One)?;
                put(one(t_use_clr)?, r.clear.is_some())?;
                if let Some(c) = r.clear {
                    put(one(t_clr)?, c == Sel::One)?;
                }
            }
        }
        let c = &self.controls;
        if c.write_enable.is_some() {
            // Set for a non-inverted write enable (every fuzz design).
            put(one(arch::RAM_WRITE_ENABLE_INVERT)?, true)?;
        }
        for k in 0..2 {
            // Clear for a non-inverted enable (s8e), set for `!wren` (h2).
            if c.enable[k].is_some() {
                put(db.feature(arch::RAM_CE_INVERT, k as i32)?.bit(0), c.enable_inverted[k])?;
            }
        }
        if c.b_write_enable.is_some() {
            put(one(arch::RAM_READ_ENABLE_INVERT)?, true)?;
        }
        let fast = [
            (arch::RAM_CLOCK_FAST_MUX, 0, c.clock[0]),
            (arch::RAM_CLOCK_FAST_MUX, 1, c.clock[1]),
            (arch::RAM_CE_FAST_MUX, 0, c.enable[0]),
            (arch::RAM_CE_FAST_MUX, 1, c.enable[1]),
            (arch::RAM_CLEAR_FAST_MUX, 0, c.clear[0]),
            (arch::RAM_CLEAR_FAST_MUX, 1, c.clear[1]),
            (arch::RAM_WE_FAST_MUX, -1, c.write_enable),
            (arch::RAM_RE_FAST_MUX, -1, c.b_write_enable),
        ];
        for (t, i, src) in fast {
            let Some(src) = src else { continue };
            let f = db.feature(t, i)?;
            // The map value is the mux's bit pattern (fuzz: clock from row
            // clock 5 = 130 = bits 1 and 7).
            let v = *f.aux.as_ref().context("fast mux map")?.get(src.index()).context("fast mux source")?;
            ensure!(v >= 0, "{t}/{i}: source {src:?} not selectable");
            for (k, &b) in f.paths[0][0].iter().enumerate() {
                put(b, v >> k & 1 == 1)?;
            }
        }
        for (k, line) in c.pia.iter().enumerate() {
            if let Some(line) = line {
                for b in db.feature(arch::RAM_PIA_TO_INPUT_YGR, k as i32)?.source_bits(0, *line as usize)? {
                    put(b, true)?;
                }
            }
        }
        for (&inp, &line) in &self.inputs {
            let (t, i) = inp.group();
            for b in db.feature(t, i)?.source_bits(0, line as usize).with_context(|| format!("{inp:?} from line {line}"))? {
                put(b, true)?;
            }
        }
        for (i, &off) in db.content.iter().enumerate() {
            put(off, self.content.get(i))?;
        }
        Ok(out)
    }
}

// ---- Quartus frontend: post-fit netlist + rcf -> M4k ----

fn param<'a>(a: &'a Atom, k: &str) -> Result<&'a str> {
    a.params.get(k).map(String::as_str).with_context(|| format!("{}: no {k}", a.name))
}

fn num(a: &Atom, k: &str) -> Result<u32> {
    param(a, k)?.parse().with_context(|| format!("{}: {k}", a.name))
}

fn sel(s: &str, what: &str) -> Result<Option<Sel>> {
    Ok(match s {
        "none" => None,
        "clock0" | "clear0" => Some(Sel::Zero),
        "clock1" | "clear1" => Some(Sel::One),
        _ => bail!("{what} = {s}"),
    })
}

fn read_port(a: &Atom, port: char) -> Result<ReadPort> {
    let w = num(a, &format!("port_{port}_address_width"))?;
    let width = Width::from_addr_bits(w).with_context(|| format!("{}: port {port} address width {w}", a.name))?;
    let clock = sel(param(a, &format!("port_{port}_data_out_clock"))?, "data_out_clock")?;
    let clear = sel(param(a, &format!("port_{port}_data_out_clear"))?, "data_out_clear")?;
    let out_reg = match (clock, clear) {
        (Some(clock), clear) => Some(OutReg { clock, clear }),
        (None, None) => None,
        (None, Some(_)) => bail!("{}: port {port} output clear without an output register", a.name),
    };
    Ok(ReadPort { width, out_reg })
}

/// `mem_init1 . mem_init0` as bits, LSB first.
fn mem_init(a: &Atom) -> Result<Vec<bool>> {
    let mut out = Vec::new();
    for k in ["mem_init0", "mem_init1"] {
        let Some(v) = a.params.get(k) else { continue };
        let (n, hex) = v.split_once("'h").with_context(|| format!("{}: {k} not hex", a.name))?;
        let n: usize = n.trim().parse()?;
        let mut bits = Vec::with_capacity(n);
        for ch in hex.trim().chars().rev() {
            let d = ch.to_digit(16).with_context(|| format!("{}: {k}", a.name))?;
            for i in 0..4 {
                bits.push(d >> i & 1 == 1);
            }
        }
        bits.resize(n, false);
        out.extend(bits);
    }
    Ok(out)
}

/// `section ram_sublocations` of the rcf: output bit -> its column slot.
/// Keys are (owner, port, bit). Quartus names an output bit one of two
/// ways: a per-bit slice `<ram>|q_b[5]` (owner `<ram>|`, the bit number of
/// the whole logical RAM), or, for RAMs whose outputs go through a depth
/// mux, the atom itself: `<atom>` (bit 0 of its first output port, port
/// `*` here) and `<atom>~PORTBDATAOUT3`. Owners are hierarchy paths with
/// the `entity:` parts dropped, which is how the post-fit netlist names the
/// atoms. Values are (half, slot): half is 0 except in a packed block,
/// where the atom on half 1 is port B.
pub type Sublocs = HashMap<(String, char, u32), (u32, u32)>;

/// `altsyncram:ram|altsyncram_t581:auto_generated|q_a[0]` ->
/// `ram|auto_generated|q_a[0]`.
pub fn instance_path(cell: &str) -> String {
    cell.split('|').map(|seg| seg.rsplit_once(':').map_or(seg, |(_, inst)| inst)).collect::<Vec<_>>().join("|")
}

/// A RAM output (or sink) cell name -> (owner, port, bit).
fn cell_key(cell: &str) -> (String, char, u32) {
    let p = instance_path(cell);
    if let Some((head, idx)) = p.strip_suffix(']').and_then(|s| s.rsplit_once('[')) {
        if let (Some(port), Ok(bit)) = (head.chars().last(), idx.parse()) {
            if let Some(owner) = head.strip_suffix(port).and_then(|h| h.strip_suffix("q_")) {
                return (owner.to_string(), port, bit);
            }
        }
    }
    for (port, tag) in [('a', "~PORTADATAOUT"), ('b', "~PORTBDATAOUT")] {
        if let Some((atom, bit)) = p.rsplit_once(tag) {
            if let Ok(bit) = bit.parse() {
                return (atom.to_string(), port, bit);
            }
        }
    }
    (p, '*', 0)
}

pub fn sublocations(rcf: &str) -> Result<Sublocs> {
    let mut out = HashMap::new();
    let Some(start) = rcf.find("section ram_sublocations") else { return Ok(out) };
    for l in rcf[start..].lines().skip(1) {
        let l = l.trim();
        if l == "}" {
            break;
        }
        // ( altsyncram:ram|...|q_b[5], 0, 4);
        let inner = l.trim_start_matches('(').trim_end_matches(';').trim_end_matches(')');
        let parts: Vec<&str> = inner.rsplitn(3, ',').map(str::trim).collect();
        ensure!(parts.len() == 3, "ram_sublocations line {l:?}");
        let (slot, field, cell) = (parts[0].parse::<u32>()?, parts[1].parse::<u32>()?, parts[2]);
        out.insert(cell_key(cell), (field, slot));
    }
    Ok(out)
}

/// The M4K a resource sits in, if any.
fn m4k_tile(x: i16, y: i16) -> Option<(u32, u32)> {
    COLUMNS.contains(&(x as u32)).then_some((x as u32, y as u32))
}

/// The RAM atoms of a post-fit netlist and their rcf routing -> blocks.
/// Also returns what isn't modelled (the block's bits are then missing).
pub fn from_quartus(atoms: &[Atom], sigs: &[Signal], rcf_text: &str) -> Result<(Vec<M4k>, Vec<String>)> {
    let mut unmodelled = Vec::new();
    let mut by_loc: BTreeMap<(u32, u32), Vec<&Atom>> = BTreeMap::new();
    for a in atoms.iter().filter(|a| a.kind == "ram_block") {
        let loc = a.loc.strip_prefix("M4K_X").and_then(|r| r.split_once("_Y")).and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)));
        by_loc.entry(loc.with_context(|| format!("{}: location {}", a.name, a.loc))?).or_default().push(a);
    }
    if by_loc.is_empty() {
        return Ok((vec![], unmodelled));
    }
    let subs = sublocations(rcf_text)?;
    let mut out = Vec::new();
    for (&(x, y), v) in &by_loc {
        let m = match v[..] {
            [a] => block(a, x, y, sigs, &subs, None).with_context(|| a.name.clone()),
            [p, q] => packed(p, q, x, y, sigs, &subs),
            _ => Err(anyhow::anyhow!("{} RAM atoms in one block", v.len())),
        };
        match m {
            Ok(m) => out.push(m),
            Err(e) => unmodelled.push(format!("M4K X{x} Y{y}: {e:#}")),
        }
    }
    Ok((out, unmodelled))
}

/// One atom's share of a block. `half` is set for one of the two
/// single-port atoms of a packed block (`packed`): the block is then 256 x 18
/// whatever the atom says, half 0 is port A on words 128-255, half 1 port B
/// on words 0-127 (fuzz s32, s36).
fn block(a: &Atom, x: u32, y: u32, sigs: &[Signal], subs: &Sublocs, half: Option<u32>) -> Result<M4k> {
    let op = param(a, "operation_mode")?;
    let mut pa = read_port(a, 'a')?;
    if half.is_some() {
        ensure!(op == "single_port", "packed {op} atom not modelled");
        pa.width = Width::X18;
    }
    let (mode, cell_port) = match op {
        // A single port reads x36 only through port B (fuzz: r36 sets port
        // B's write mode and read clock, its address enters on the read
        // address pins).
        "rom" if pa.width == Width::X36 => (Mode::SimpleDual { write: None, read: pa, read_clock: Sel::Zero }, 'a'),
        "single_port" | "rom" => (Mode::Single { a: pa }, 'a'),
        "dual_port" => {
            let read = read_port(a, 'b')?;
            let read_clock = sel(param(a, "port_b_address_clock")?, "port_b_address_clock")?.context("port B address unclocked")?;
            (Mode::SimpleDual { write: Some(pa.width), read, read_clock }, 'b')
        }
        "bidir_dual_port" => {
            let b = read_port(a, 'b')?;
            let b_clock = sel(param(a, "port_b_address_clock")?, "port_b_address_clock")?.context("port B address unclocked")?;
            (Mode::true_dual(pa, b, b_clock)?, 'a')
        }
        m => bail!("operation_mode {m} not modelled"),
    };
    // A RAM split over blocks: each atom holds its own slice of the words
    // (mem_init is per atom) and of the bits (its first bit number offsets
    // the rcf's per-bit slice names).
    let first = |p: char| -> u32 { a.params.get(&format!("port_{p}_first_bit_number")).and_then(|v| v.parse().ok()).unwrap_or(0) };
    let prefix = match a.name.rfind('|') {
        Some(i) => a.name[..=i].to_string(),
        None => String::new(),
    };
    // An atom's bits need not be contiguous slices of the RAM: a x36 RAM
    // split into two x18 atoms gives one of them bits 0-8 and 27-35 (s36,
    // w13), whatever its first bit number says. Atom bit k is the k-th of
    // its slices: in a packed block the sublocations on its half, otherwise
    // the output nets leaving this block's M4K_BUFFERs, if every bit has one.
    let mut cells: HashMap<char, Vec<u32>> = HashMap::new();
    if let Some(h) = half {
        let v = subs.iter().filter(|((o, p, _), (hh, _))| *o == prefix && *p == 'a' && *hh == h).map(|((_, _, b), _)| *b);
        cells.insert('a', v.collect());
    } else {
        for s in sigs {
            let Some(r) = s.resources.first() else { continue };
            if r.kind == "M4K_BUFFER" && m4k_tile(r.x, r.y) == Some((x, y)) {
                let (owner, p, b) = cell_key(&s.name);
                if owner == prefix {
                    cells.entry(p).or_default().push(b);
                }
            }
        }
        cells.retain(|p, v| a.params.get(&format!("port_{p}_data_width")).and_then(|w| w.parse::<usize>().ok()) == Some(v.len()));
    }
    for v in cells.values_mut() {
        v.sort();
        v.dedup();
    }
    // Slice name (bit number of the logical RAM) of atom bit `bit`, and back.
    let slice_of = |p: char, bit: u32| -> u32 {
        match cells.get(&p) {
            Some(c) => c.get(bit as usize).copied().unwrap_or(u32::MAX),
            None => bit + first(p),
        }
    };
    let atom_bit = |p: char, slice: u32| -> Option<u32> {
        match cells.get(&p) {
            Some(c) => c.iter().position(|&b| b == slice).map(|i| i as u32),
            None => slice.checked_sub(first(p)),
        }
    };
    let widths = |p: char| -> Result<Width> {
        Ok(match (mode, p) {
            (Mode::Single { a }, 'a') => a.width,
            (Mode::SimpleDual { write: Some(w), .. }, 'a') => w,
            (Mode::SimpleDual { write: None, read, .. }, 'a') => read.width,
            (Mode::SimpleDual { read, .. }, 'b') => read.width,
            (Mode::TrueDual { a, .. }, 'a') => a.width,
            (Mode::TrueDual { b, .. }, 'b') => b.width,
            _ => bail!("port {p} unused in {mode:?}"),
        })
    };
    let cell_width = widths(cell_port)?;
    // A sublocation slot is a column of the narrower port's word; bit k of
    // a wider port's slice sits in that port's (k / narrow width)-th narrow
    // word (fuzz: x8 write / x32 read lists slots 0-7 four times).
    let ports: &[char] = if matches!(mode, Mode::Single { .. }) { &['a'] } else { &['a', 'b'] };
    let mut narrow_port = 'a';
    for &p in ports {
        if widths(p)? < widths(narrow_port)? {
            narrow_port = p;
        }
    }
    let narrow = widths(narrow_port)?;
    let narrow_bits = num(a, &format!("port_{narrow_port}_data_width"))? as usize;
    // quartus_map keeps an atom from a VQM whole, named after its first
    // output net; its own atoms sit inside the altsyncram's hierarchy.
    let vqm_atom = half.is_none() && !a.name.contains('|');
    // Linear position, within one word of the widest port, of bit `bit` of
    // a slice, `idx` narrow words further on (a wide port's data pins on a
    // narrow slice are `PORTADATAIN[idx]`).
    let column = |port: char, bit: u32, idx: u32| -> Result<usize> {
        let key = |owner: &str, p: char, b: u32| subs.get(&(owner.to_string(), p, b));
        // An atom read from a VQM (`cram quartus-export`) isn't split into
        // per-bit slices: the rcf lists its output nets, each at (0, 0), and
        // bit k sits in column k of the narrow word (VQM experiment q1:
        // output k on M4K_BUFFER k, contents in that order).
        let (_, s) = key(&prefix, port, slice_of(port, bit))
            .or_else(|| key(&a.name, port, bit))
            .or_else(|| if bit == 0 { key(&a.name, '*', 0) } else { None })
            .or_else(|| key(&prefix, cell_port, slice_of(cell_port, bit)))
            .copied()
            .or_else(|| vqm_atom.then_some((0, bit % narrow_bits as u32)))
            .with_context(|| format!("no ram_sublocation for port {port} bit {bit}"))?;
        // A packed half's slots count both halves' 36 columns.
        let s = if half.is_some() { s as usize % 18 } else { s as usize };
        Ok(narrow.linear(bit as usize / narrow_bits + idx as usize, s))
    };

    // Contents: mem_init is in the words of the port whose outputs the rcf
    // names (port A for single port, B for simple dual); fit on every fuzz
    // design.
    let mut content = Content::default();
    let w = num(a, &format!("port_{cell_port}_data_width"))? as usize;
    let word0 = if half == Some(0) { 128 } else { 0 };
    for (n, &b) in mem_init(a)?.iter().enumerate() {
        if b {
            content.set_linear(cell_width.linear(word0 + n / w, column(cell_port, (n % w) as u32, 0)?));
        }
    }

    // Inputs and controls from the rcf: every sink whose last resource is in
    // this block's tile.
    let mut m = M4k { x, y, mode, controls: Controls::default(), inputs: BTreeMap::new(), content };
    for s in sigs {
        for d in &s.dests {
            let Some(r) = d.from.map(|i| &s.resources[i]) else { continue };
            if m4k_tile(r.x, r.y) != Some((x, y)) {
                continue;
            }
            let (port, idx) = match d.port.split_once('[') {
                Some((p, i)) => (p, i.trim_end_matches(']').parse::<u32>()?),
                None => (d.port.as_str(), 0),
            };
            // Which data bit a data/byte-enable sink is: on a per-bit slice
            // cell, the slice's bit (and `idx` narrow words on); on an atom
            // cell, bit `idx` of the port.
            let (owner, cport, cbit) = cell_key(&d.cell);
            let port_idx = idx;
            let (cbit, idx) = if owner == a.name || vqm_atom {
                (idx, 0)
            } else {
                match atom_bit(cport, cbit) {
                    Some(b) => (b, idx),
                    // The other atom of a packed block.
                    None if half.is_some() => continue,
                    None => bail!("slice {cbit} below the atom's first bit"),
                }
            };
            let line = || -> Result<u8> {
                ensure!(r.kind == "LOCAL_INTERCONNECT", "{} {}: fed by {}", d.cell, d.port, r.kind);
                Ok(r.index as u8)
            };
            let ctl = || -> Result<(CtlSrc, Option<(usize, u8)>)> {
                Ok(match r.kind.as_str() {
                    "BLK_CLK_BUF" => (CtlSrc::Global(r.index as u8), None),
                    k if k.ends_with("CONTROL_INPUT") => {
                        let prev = s.parents[d.from.unwrap()].map(|p| &s.resources[p]);
                        let prev = prev.filter(|p| p.kind == "LOCAL_INTERCONNECT").with_context(|| format!("{k} not fed by a LAB line"))?;
                        let k = r.index as usize;
                        ensure!(k < 8, "control input {k}");
                        (CtlSrc::Control(k as u8), Some((k, prev.index as u8)))
                    }
                    k => bail!("{} {}: control from {k}", d.cell, d.port),
                })
            };
            match port {
                "PORTAADDR" | "PORTBADDR" => {
                    let b = port == "PORTBADDR" || matches!(mode, Mode::SimpleDual { write: None, .. });
                    let w = widths(if b && half.is_none() { 'b' } else { 'a' })?;
                    let inp = Input::addr(b || half == Some(1), port_idx + 12 - w.addr_bits())?;
                    put_input(&mut m.inputs, inp, line()?)?;
                }
                "PORTADATAIN" | "PORTBDATAIN" => {
                    // Bit idx of the slice named by the cell: the idx-th
                    // word of the cell's port inside one word of this port.
                    let b = port == "PORTBDATAIN";
                    let w = widths(if b { 'b' } else { 'a' })?;
                    ensure!(cport == cell_port || owner == a.name || vqm_atom, "data sink on cell of port {cport}");
                    let col = w.column(column(if b { 'b' } else { 'a' }, cbit, idx)?);
                    let inp = Input::Data((col + if b || half == Some(1) { 18 } else { 0 }) as u8);
                    put_input(&mut m.inputs, inp, line()?)?;
                }
                "PORTABYTEENAMASKS" => {
                    // On an atom cell the index is the byte, not a bit.
                    let byte = if owner == a.name || vqm_atom { port_idx as usize } else { widths('a')?.column(column('a', cbit, idx)?) / 9 };
                    let inp = Input::ByteEna(byte as u8);
                    put_input(&mut m.inputs, inp, line()?)?;
                }
                "PORTBREWE" if !matches!(mode, Mode::TrueDual { .. }) => bail!("port B read enable not modelled"),
                "PORTAWE" | "PORTBREWE" | "CLK0" | "CLK1" | "ENA0" | "ENA1" | "CLR0" | "CLR1" => {
                    let (src, pia) = ctl()?;
                    if let Some((k, l)) = pia {
                        put(&mut m.controls.pia[k], l, "control input")?;
                    }
                    let c = &mut m.controls;
                    let (slot, what) = match port {
                        "PORTAWE" if half == Some(1) => (&mut c.b_write_enable, "port B write enable"),
                        "PORTAWE" => (&mut c.write_enable, "write enable"),
                        "PORTBREWE" => (&mut c.b_write_enable, "port B write enable"),
                        "CLK0" => (&mut c.clock[0], "clock"),
                        "CLK1" => (&mut c.clock[1], "clock"),
                        "ENA0" => (&mut c.enable[0], "clock enable"),
                        "ENA1" => (&mut c.enable[1], "clock enable"),
                        "CLR0" => (&mut c.clear[0], "clear"),
                        _ => (&mut c.clear[1], "clear"),
                    };
                    put(slot, src, what)?;
                }
                p => bail!("RAM port {p} not modelled"),
            }
        }
    }
    for p in ["portawe", "portbrewe", "clk0", "clk1", "clr0", "clr1"] {
        ensure!(!a.port(p).is_some_and(|n| n.starts_with('!')), "inverted {p} not modelled");
    }
    for k in 0..2 {
        m.controls.enable_inverted[k] = m.controls.enable[k].is_some() && a.port(&format!("ena{k}")).is_some_and(|n| n.starts_with('!'));
    }
    for p in ["portaaddrstall", "portbaddrstall"] {
        ensure!(a.net(p).is_none(), "{p} not modelled");
    }
    Ok(m)
}

/// Two single-port atoms sharing one block (packed mode). Which is port A
/// is the rcf's sublocation half of their output bits.
fn packed(p: &Atom, q: &Atom, x: u32, y: u32, sigs: &[Signal], subs: &Sublocs) -> Result<M4k> {
    let half_of = |a: &Atom| -> Result<u32> {
        let prefix = a.name.rfind('|').map_or("", |i| &a.name[..=i]);
        let first: u32 = a.params.get("port_a_first_bit_number").and_then(|v| v.parse().ok()).unwrap_or(0);
        let (h, _) = subs.get(&(prefix.to_string(), 'a', first)).with_context(|| format!("{}: no sublocation", a.name))?;
        Ok(*h)
    };
    let (a0, a1) = match (half_of(p)?, half_of(q)?) {
        (0, 1) => (p, q),
        (1, 0) => (q, p),
        h => bail!("packed atoms on halves {h:?}"),
    };
    let m0 = block(a0, x, y, sigs, subs, Some(0)).with_context(|| a0.name.clone())?;
    let m1 = block(a1, x, y, sigs, subs, Some(1)).with_context(|| a1.name.clone())?;
    let out = |m: &M4k| match m.mode {
        Mode::Single { a } => Ok(a.out_reg),
        _ => bail!("packed half is not single port"),
    };
    let mut m = M4k { mode: Mode::Packed { a_out: out(&m0)?, b_out: out(&m1)? }, ..m0.clone() };
    for (&inp, &line) in &m1.inputs {
        put_input(&mut m.inputs, inp, line)?;
    }
    let (c, d) = (&mut m.controls, &m1.controls);
    for k in 0..8 {
        if let Some(l) = d.pia[k] {
            put(&mut c.pia[k], l, "control input")?;
        }
    }
    for k in 0..2 {
        for (slot, v) in [(&mut c.clock[k], d.clock[k]), (&mut c.enable[k], d.enable[k]), (&mut c.clear[k], d.clear[k])] {
            if let Some(v) = v {
                put(slot, v, "shared control")?;
            }
        }
        c.enable_inverted[k] |= d.enable_inverted[k];
    }
    if let Some(v) = d.b_write_enable {
        put(&mut c.b_write_enable, v, "port B write enable")?;
    }
    for (w, &v) in m.content.0.iter_mut().zip(m1.content.0.iter()) {
        *w |= v;
    }
    Ok(m)
}

fn put<T: PartialEq + std::fmt::Debug + Copy>(slot: &mut Option<T>, v: T, what: &str) -> Result<()> {
    match slot {
        Some(old) if *old != v => bail!("two different {what} sources ({old:?}, {v:?})"),
        _ => *slot = Some(v),
    }
    Ok(())
}

fn put_input(inputs: &mut BTreeMap<Input, u8>, inp: Input, line: u8) -> Result<()> {
    match inputs.insert(inp, line) {
        Some(old) if old != line => bail!("{inp:?} fed from lines {old} and {line}"),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Content index (`col * 128 + row`) of word `a` bit `s`.
    fn index(w: Width, a: usize, s: usize) -> usize {
        let l = w.linear(a, s);
        (l % COLS) * ROWS + l / COLS
    }

    #[test]
    fn content_layout_matches_x8_linear_fuzz() {
        // lin0-lin11 (512 x 8, one design per bit of a*8+b): content index
        // col*128 + row holds word 4*row + col/9, bit col%9; columns 8, 17,
        // 26, 35 (the ninth bits) are never set.
        assert_eq!(index(Width::X9, 0, 0), 0);
        assert_eq!(index(Width::X9, 4, 0), 1);
        assert_eq!(index(Width::X9, 0, 1), 128);
        assert_eq!(index(Width::X9, 1, 0), 9 * 128);
        assert_eq!(index(Width::X9, 2, 0), 18 * 128);
        // x1 skips the ninth columns too; x18 and x36 don't.
        assert_eq!(index(Width::X1, 8, 0), 9 * 128);
        assert_eq!(index(Width::X18, 1, 0), 18 * 128);
        assert_eq!(index(Width::X36, 1, 35), 35 * 128 + 1);
    }

    #[test]
    fn address_pins() {
        // Port A x8 (9 address bits): bit 0 on data input 15, bit 1 on write
        // address 11, bits 2-8 on write address 0-6 (a0, s8*, d8).
        assert_eq!(Input::addr(false, 3).unwrap(), Input::Data(15));
        assert_eq!(Input::addr(false, 4).unwrap(), Input::WriteAddr(11));
        assert_eq!(Input::addr(false, 11).unwrap(), Input::WriteAddr(6));
        // Port B x1 (d1): bits 0, 1 on data inputs 26, 35, then read
        // address 9, 10, 11, 0-6.
        assert_eq!(Input::addr(true, 0).unwrap(), Input::Data(26));
        assert_eq!(Input::addr(true, 1).unwrap(), Input::Data(35));
        assert_eq!(Input::addr(true, 2).unwrap(), Input::ReadAddr(9));
        assert_eq!(Input::addr(true, 5).unwrap(), Input::ReadAddr(0));
    }

    #[test]
    fn parses_sublocations() {
        let s = sublocations("x\nsection ram_sublocations {\n ( altsyncram:ram|altsyncram_tbg1:auto_generated|q_b[2], 0, 1);\n}\n").unwrap();
        assert_eq!(s.get(&("ram|auto_generated|".to_string(), 'b', 2)), Some(&(0, 1)));
        assert_eq!(cell_key("altsyncram:ram|altsyncram_8032:auto_generated|ram_block1a1~PORTBDATAOUT0"), ("ram|auto_generated|ram_block1a1".to_string(), 'b', 0));
    }
}
