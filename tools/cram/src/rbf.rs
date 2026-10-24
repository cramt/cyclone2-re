//! Uncompressed EP2C35 raw bitstream (`.rbf`) container, as observed from
//! Quartus 13.0sp1 output (README "Container format").

use anyhow::{ensure, Context, Result};

pub const HEADER_BYTES: usize = 0x186;
pub const FRAME_DATA_BYTES: usize = 312;
pub const FRAME_BYTES: usize = FRAME_DATA_BYTES + 2;
pub const FRAMES: usize = 2725;
pub const TAIL_BYTES: usize = 36;
pub const TOTAL_BYTES: usize = HEADER_BYTES + FRAMES * FRAME_BYTES + TAIL_BYTES;
pub const FRAME_BITS: usize = FRAME_DATA_BYTES * 8;
/// Raw frames 0-23 aren't 24 frames: they are 6 ordinary 314-byte frames
/// (312 data bytes + the same CRC-16 as every other frame) with every byte
/// sent 4 times in a row. Checked on every uncompressed RBF we have. So
/// the stream really holds 6 + 2701 frames; `FrameBit` keeps addressing
/// raw frames, `RepBit` the 6 replicated ones.
pub const FIRST_CRC_FRAME: usize = 24;
pub const REP_FRAMES: usize = 6;
pub const REP_COPIES: usize = 4;

/// Header bytes covered by the header check word, and where it is stored.
const HDR_CRC_RANGE: std::ops::Range<usize> = 0x4d..0x75;
const HDR_CRC_AT: usize = 0x75;

/// A data bit: `bit` counts LSB first within the frame's 312 data bytes,
/// i.e. byte `bit / 8`, bit `bit % 8`, the same order rbfdiff prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrameBit {
    pub frame: u16,
    pub bit: u16,
}

impl std::fmt::Display for FrameBit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "f{}:{}", self.frame, self.bit)
    }
}

/// Reflected CRC-16, poly 0xA001 (ARC/MODBUS).
fn crc16(init: u16, data: &[u8]) -> u16 {
    let mut c = init;
    for &b in data {
        c ^= b as u16;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xa001 } else { c >> 1 };
        }
    }
    c
}

/// Frame check init 0xfe54 is "effective": it's what makes all 2701 frames
/// of every observed design check, not a constant found in Quartus.
fn frame_crc(data: &[u8]) -> u16 {
    crc16(0xfe54, data)
}

/// A data bit of one of the 6 replicated frames, LSB first like `FrameBit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepBit {
    pub frame: u8,
    pub bit: u16,
}

impl std::fmt::Display for RepBit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "x{}:{}", self.frame, self.bit)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Rbf(Vec<u8>);

impl Rbf {
    pub fn parse(bytes: Vec<u8>) -> Result<Self> {
        ensure!(
            bytes.len() == TOTAL_BYTES,
            "{} bytes, an uncompressed EP2C35 RBF is {TOTAL_BYTES}",
            bytes.len()
        );
        Ok(Rbf(bytes))
    }
    pub fn read(path: &std::path::Path) -> Result<Self> {
        Self::parse(std::fs::read(path).with_context(|| path.display().to_string())?)
    }
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    fn frame_off(frame: usize) -> usize {
        HEADER_BYTES + frame * FRAME_BYTES
    }
    pub fn frame_data(&self, frame: usize) -> &[u8] {
        let o = Self::frame_off(frame);
        &self.0[o..o + FRAME_DATA_BYTES]
    }
    pub fn get(&self, fb: FrameBit) -> bool {
        let b = self.frame_data(fb.frame as usize)[fb.bit as usize / 8];
        b >> (fb.bit % 8) & 1 == 1
    }
    pub fn set(&mut self, fb: FrameBit, v: bool) {
        let o = Self::frame_off(fb.frame as usize) + fb.bit as usize / 8;
        let m = 1u8 << (fb.bit % 8);
        if v {
            self.0[o] |= m
        } else {
            self.0[o] &= !m
        }
    }

    /// Data and check word of one frame.
    pub fn frame_bytes(&self, frame: usize) -> &[u8] {
        let o = Self::frame_off(frame);
        &self.0[o..o + FRAME_BYTES]
    }
    pub fn set_frame_bytes(&mut self, frame: usize, bytes: &[u8]) {
        let o = Self::frame_off(frame);
        self.0[o..o + FRAME_BYTES].copy_from_slice(bytes);
    }

    /// Offset of the first copy of byte `byte` (0..314, check word
    /// included) of replicated frame `frame`.
    fn rep_off(frame: usize, byte: usize) -> usize {
        HEADER_BYTES + (frame * FRAME_BYTES + byte) * REP_COPIES
    }
    /// One replicated frame, de-replicated: 312 data bytes + check word.
    pub fn rep_frame(&self, frame: usize) -> Vec<u8> {
        (0..FRAME_BYTES).map(|b| self.0[Self::rep_off(frame, b)]).collect()
    }
    fn set_rep_byte(&mut self, frame: usize, byte: usize, v: u8) {
        let o = Self::rep_off(frame, byte);
        self.0[o..o + REP_COPIES].fill(v);
    }
    pub fn rep_get(&self, b: RepBit) -> bool {
        self.0[Self::rep_off(b.frame as usize, b.bit as usize / 8)] >> (b.bit % 8) & 1 == 1
    }
    pub fn rep_set(&mut self, b: RepBit, v: bool) {
        let (f, byte) = (b.frame as usize, b.bit as usize / 8);
        let old = self.0[Self::rep_off(f, byte)];
        let m = 1u8 << (b.bit % 8);
        self.set_rep_byte(f, byte, if v { old | m } else { old & !m });
    }
    /// Set an option bit of the header's ID block (0x4c..0x75, bits 0-1;
    /// `fix_crcs` updates its check word).
    pub fn set_option(&mut self, byte: usize, bit: u8) {
        assert!((0x4c..HDR_CRC_AT).contains(&byte) && bit < 2, "not an ID-block option bit");
        self.0[byte] |= 1 << bit;
    }
    /// Are all 4 copies of every byte equal and is the check word right?
    pub fn rep_frame_ok(&self, frame: usize) -> bool {
        let copies_ok = (0..FRAME_BYTES).all(|b| {
            let o = Self::rep_off(frame, b);
            self.0[o..o + REP_COPIES].iter().all(|&x| x == self.0[o])
        });
        let d = self.rep_frame(frame);
        copies_ok && frame_crc(&d[..FRAME_DATA_BYTES]) == u16::from_le_bytes([d[312], d[313]])
    }
    /// Replicated-frame data bits that differ.
    pub fn rep_diff(&self, other: &Rbf) -> Vec<RepBit> {
        let mut v = Vec::new();
        for f in 0..REP_FRAMES {
            let (a, b) = (self.rep_frame(f), other.rep_frame(f));
            for i in 0..FRAME_DATA_BYTES {
                let x = a[i] ^ b[i];
                for k in 0..8 {
                    if x >> k & 1 == 1 {
                        v.push(RepBit { frame: f as u8, bit: (i * 8 + k) as u16 });
                    }
                }
            }
        }
        v
    }

    /// Does frame `frame`'s stored check word match its data?
    pub fn frame_crc_ok(&self, frame: usize) -> bool {
        let o = Self::frame_off(frame) + FRAME_DATA_BYTES;
        frame_crc(self.frame_data(frame)) == u16::from_le_bytes([self.0[o], self.0[o + 1]])
    }

    /// Recompute every check word: the 6 replicated frames, frames
    /// 24..2724 and the header.
    pub fn fix_crcs(&mut self) {
        for f in 0..REP_FRAMES {
            let d = self.rep_frame(f);
            let c = frame_crc(&d[..FRAME_DATA_BYTES]).to_le_bytes();
            self.set_rep_byte(f, FRAME_DATA_BYTES, c[0]);
            self.set_rep_byte(f, FRAME_DATA_BYTES + 1, c[1]);
        }
        for f in FIRST_CRC_FRAME..FRAMES {
            let c = frame_crc(self.frame_data(f));
            let o = Self::frame_off(f) + FRAME_DATA_BYTES;
            self.0[o..o + 2].copy_from_slice(&c.to_le_bytes());
        }
        let c = crc16(0xffff, &self.0[HDR_CRC_RANGE]);
        self.0[HDR_CRC_AT..HDR_CRC_AT + 2].copy_from_slice(&c.to_le_bytes());
    }

    /// Data bits that differ, raw frames in order (a replicated-frame bit
    /// shows up 4 times here; see `rep_diff`).
    pub fn diff(&self, other: &Rbf) -> Vec<FrameBit> {
        let mut v = Vec::new();
        for f in 0..FRAMES {
            for (i, (a, b)) in self.frame_data(f).iter().zip(other.frame_data(f)).enumerate() {
                let x = a ^ b;
                for k in 0..8 {
                    if x >> k & 1 == 1 {
                        v.push(FrameBit { frame: f as u16, bit: (i * 8 + k) as u16 });
                    }
                }
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_zero_frame_check_word() {
        // README: an all-zero frame's stored word is `ef 32`.
        assert_eq!(frame_crc(&[0u8; FRAME_DATA_BYTES]).to_le_bytes(), [0xef, 0x32]);
    }

    #[test]
    fn replicated_frames_round_trip() {
        let mut r = Rbf(vec![0; TOTAL_BYTES]);
        let b = RepBit { frame: 3, bit: 1234 };
        r.rep_set(b, true);
        r.fix_crcs();
        assert!(r.rep_get(b));
        assert!((0..REP_FRAMES).all(|f| r.rep_frame_ok(f)));
        assert_eq!(r.rep_diff(&Rbf(vec![0; TOTAL_BYTES])), vec![b]);
        // Byte 154 of frame 3 sits at 4 consecutive raw bytes.
        let o = HEADER_BYTES + (3 * FRAME_BYTES + 154) * REP_COPIES;
        assert_eq!(&r.0[o..o + 4], &[4, 4, 4, 4]);
    }
}
