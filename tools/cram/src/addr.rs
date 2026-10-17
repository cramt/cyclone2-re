//! Flat CRAM address -> RBF frame and bit. This is the whole of "bitstream
//! addressing" for routing bits: the device database (asm) speaks in flat
//! addresses, the RBF in (frame, bit-in-frame).
//!
//! Fit against routing fuzz designs (README "CRAM addressing"):
//!
//!   column = flat % 2701, row = flat / 2701
//!   frame  = 2724 - column                     (frames 24..2724)
//!   lane   = row / 311, pos = row % 311
//!   byte   = 311 - pos  (lane 7: 310 - pos)
//!   bit    = byte * 8 + lane

use crate::rbf::FrameBit;

pub const COLUMNS: u32 = 2701;
const LAST_FRAME: u32 = 2724;
const LANE_ROWS: u32 = 311;

pub fn frame_bit(flat: u32) -> FrameBit {
    let (row, col) = (flat / COLUMNS, flat % COLUMNS);
    let (lane, pos) = (row / LANE_ROWS, row % LANE_ROWS);
    let byte = if lane == 7 { 310 - pos } else { 311 - pos };
    FrameBit { frame: (LAST_FRAME - col) as u16, bit: (byte * 8 + lane) as u16 }
}

/// Inverse of `frame_bit`; `None` for frames 0-23 and for (byte, lane)
/// slots no flat address maps to.
pub fn flat(fb: FrameBit) -> Option<u32> {
    let frame = fb.frame as u32;
    if !(LAST_FRAME + 1 - COLUMNS..=LAST_FRAME).contains(&frame) {
        return None;
    }
    let (byte, lane) = (fb.bit as u32 / 8, fb.bit as u32 % 8);
    let top: u32 = if lane == 7 { 310 } else { 311 };
    let pos = top.checked_sub(byte).filter(|&p| p < LANE_ROWS)?;
    Some((lane * LANE_ROWS + pos) * COLUMNS + (LAST_FRAME - frame))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for flat_addr in [6715u32, 3238782, 6707826, 2701 * 2176, 2701 * 2177 + 5] {
            assert_eq!(flat(frame_bit(flat_addr)), Some(flat_addr), "{flat_addr}");
        }
    }
}
