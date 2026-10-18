//! Logic element geometry: where LUT bits sit in CRAM, in the (frame, row)
//! space of `addr` (row = flat address / 2701). All of it fit on the routing
//! fuzz designs (README "LE geometry"); no Quartus data names these.

/// LAB row Y (1..=35) owns 70 CRAM rows starting here. Rows 0-17 and the
/// rows after Y=1 belong to the top and bottom I/O rows.
pub fn lab_row(y: u32) -> u32 {
    18 + 70 * (35 - y)
}

pub const LAB_ROWS: u32 = 70;

/// Column widths in frames between consecutive LAB columns' LUTs: 35 per
/// LAB column, 166 per M4K/multiplier column (13, 26, 52), 27 for the
/// centre clock column (39), and 4 unexplained extra frames before columns
/// 16 and 50 (mirror images about the die centre).
pub fn lut_frame(x: u32) -> Option<u32> {
    let width = |c: u32| match c {
        13 | 26 | 52 => 166,
        39 => 27,
        _ => 35,
    };
    if !(1..=64).contains(&x) || width(x) != 35 {
        return None;
    }
    let extra = 4 * [16, 50].iter().filter(|&&c| c <= x).count() as u32;
    Some(78 + (1..x).map(width).sum::<u32>() + extra)
}

/// Physical LUT input (A..D = 0..3) values for truth-table index p.
fn inputs(p: u32) -> (u32, u32, u32, u32) {
    (p & 1, p >> 1 & 1, p >> 2 & 1, p >> 3 & 1)
}

/// (frame, row) of physical truth-table entry `p` (= A + 2B + 4C + 8D) of
/// the LUT in LCCOMB_X{x}_Y{y}_N{n}. Inputs B and C pick the frame, A and D
/// the row. LEs 8-15 are mirror images of LEs 0-7 about the LAB's centre.
pub fn lut_bit(x: u32, y: u32, n: u32, p: u32) -> Option<(u32, u32)> {
    let le = n / 2;
    if n % 2 != 0 || le > 15 || !(1..=35).contains(&y) || p > 15 {
        return None;
    }
    let (a, b, c, d) = inputs(p);
    let frame = lut_frame(x)? + 3 - b - 2 * c;
    let off = 4 * (le % 8) + 3 - a - 2 * d;
    let row = lab_row(y) + if le < 8 { off } else { LAB_ROWS - 1 - off };
    Some((frame, row))
}

/// (frame, row) of an LE-relative config bit: `df` frames from the column's
/// first LUT frame, row `r` (0..4) within the LE, counted the way LEs 0-7
/// lay out (LEs 8-15 are mirrored).
pub fn le_bit(x: u32, y: u32, n: u32, df: i32, r: u32) -> Option<(u32, u32)> {
    let le = n / 2;
    if n % 2 != 0 || le > 15 || !(1..=35).contains(&y) || r > 3 {
        return None;
    }
    let frame = (lut_frame(x)? as i32 + df) as u32;
    let off = 4 * (le % 8) + r;
    Some((frame, lab_row(y) + if le < 8 { off } else { LAB_ROWS - 1 - off }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lut_frames_match_fuzz_data() {
        // First LUT frame per LAB column seen in the fuzz designs.
        for (x, f) in [(1, 78), (12, 463), (14, 664), (15, 699), (16, 738), (25, 1053), (27, 1254),
                       (38, 1639), (40, 1701), (49, 2016), (50, 2055), (51, 2090), (53, 2291), (64, 2676)] {
            assert_eq!(lut_frame(x), Some(f), "x{x}");
        }
    }

    #[test]
    fn phase1_lut_table() {
        // README phase-1 table: X64_Y19_N0, p = A+2B+4C+8D -> frame 2679-B-2C,
        // frame bit 827 + 8A + 16D. Byte b of lane 3 is row 3*311 + 311 - b.
        for p in 0..16 {
            let (a, b, c, d) = inputs(p);
            let bit = 827 + 8 * a + 16 * d;
            let row = 3 * 311 + 311 - bit / 8;
            assert_eq!(lut_bit(64, 19, 0, p), Some((2679 - b - 2 * c, row)));
        }
    }
}
