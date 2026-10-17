//! Flat CRAM address -> RBF frame and bit. This is the whole of "bitstream
//! addressing" for routing bits: the device database (asm) speaks in flat
//! addresses, the RBF in (frame, bit-in-frame).
//!
//! Fit against routing fuzz designs (README "CRAM addressing"):
//!
//!   column = flat % 2701, row = flat / 2701
//!   frame  = 2724 - column                     (frames 24..2724)
//!   rows 0..2175:   lane = row / 311, byte = 311 - row % 311
//!   rows 2176..:    lane = 7,         byte = 311 - (row - 2176)
//!   bit    = byte * 8 + lane
//!
//! So each bit-in-byte "lane" is a run of rows laid out from the end of the
//! frame backwards. Lane 6 is one row short (ends at byte 2), which is what
//! the data says (rows 2175 and 2176 both observed); why is open.

use crate::rbf::FrameBit;

pub const COLUMNS: u32 = 2701;
pub const LAST_FRAME: u32 = 2724;
const LANE_ROWS: u32 = 311;
const LANE7_ROW: u32 = 2176;

pub fn frame_bit(flat: u32) -> FrameBit {
    let (row, col) = (flat / COLUMNS, flat % COLUMNS);
    let (lane, pos) = if row >= LANE7_ROW { (7, row - LANE7_ROW) } else { (row / LANE_ROWS, row % LANE_ROWS) };
    let byte = 311 - pos;
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
    let pos = 311 - byte;
    let row = match lane {
        7 => LANE7_ROW + pos,
        6 if pos >= LANE7_ROW - 6 * LANE_ROWS => return None,
        _ if pos >= LANE_ROWS => return None,
        _ => lane * LANE_ROWS + pos,
    };
    Some(row * COLUMNS + (LAST_FRAME - frame))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for flat_addr in [6715u32, 3238782, 6707826, 2701 * 2175, 2701 * 2176 + 5, 2701 * 1865, 2701 * 1866] {
            assert_eq!(flat(frame_bit(flat_addr)), Some(flat_addr), "{flat_addr}");
        }
    }
}
