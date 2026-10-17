//! Uncompressed EP2C35 raw bitstream (`.rbf`) container, as observed from
//! Quartus 13.0sp1 output (README "Container format").

use anyhow::{ensure, Result};

pub const HEADER_BYTES: usize = 0x186;
pub const FRAME_DATA_BYTES: usize = 312;
pub const FRAME_BYTES: usize = FRAME_DATA_BYTES + 2;
pub const FRAMES: usize = 2725;
pub const TAIL_BYTES: usize = 36;
pub const TOTAL_BYTES: usize = HEADER_BYTES + FRAMES * FRAME_BYTES + TAIL_BYTES;
pub const FRAME_BITS: usize = FRAME_DATA_BYTES * 8;
/// Frames 0-23 carry a check word we can't compute yet (probably the left
/// I/O column with its own scheme), so they're never rewritten.
pub const FIRST_CRC_FRAME: usize = 24;

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
        Self::parse(std::fs::read(path)?)
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

    /// Does frame `frame`'s stored check word match its data?
    pub fn frame_crc_ok(&self, frame: usize) -> bool {
        let o = Self::frame_off(frame) + FRAME_DATA_BYTES;
        frame_crc(self.frame_data(frame)) == u16::from_le_bytes([self.0[o], self.0[o + 1]])
    }

    /// Recompute every computable check word: frames 24..2724 and the
    /// header. Frames 0-23 are left as they are.
    pub fn fix_crcs(&mut self) {
        for f in FIRST_CRC_FRAME..FRAMES {
            let c = frame_crc(self.frame_data(f));
            let o = Self::frame_off(f) + FRAME_DATA_BYTES;
            self.0[o..o + 2].copy_from_slice(&c.to_le_bytes());
        }
        let c = crc16(0xffff, &self.0[HDR_CRC_RANGE]);
        self.0[HDR_CRC_AT..HDR_CRC_AT + 2].copy_from_slice(&c.to_le_bytes());
    }

    /// Data bits that differ, frames in order.
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
}
