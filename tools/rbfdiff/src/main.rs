//! `rbfdiff [--no-crc] a.rbf b.rbf` prints every bit that differs between two
//! uncompressed EP2C35 raw bitstreams, one per line:
//!
//!     <stream bit> <where> <a>-><b>
//!
//! Stream bit is `byte * 8 + bit` with bit 0 the LSB, because passive serial
//! shifts each RBF byte into the device LSB first; that makes the stream bit
//! the order the configuration shift register actually sees.
//!
//! `<where>` places the bit in the container layout found in phase 1 (see
//! README): `hdr+<bit>`, `f<frame>:<bit>` for data, `f<frame>:crc<bit>` for a
//! frame's trailing check word, or `tail+<bit>`. `--no-crc` drops check-word
//! bits, which always move along with any data change and only add noise.

use std::process::ExitCode;

/// EP2C35 container layout, observed from Quartus 13.0sp1 RBFs.
mod layout {
    pub const HEADER_BYTES: usize = 0x186;
    pub const FRAME_BYTES: usize = 314;
    pub const FRAME_DATA_BYTES: usize = 312;
    pub const FRAMES: usize = 2725;
    pub const TAIL_BYTES: usize = 36;
    pub const TOTAL_BYTES: usize = HEADER_BYTES + FRAMES * FRAME_BYTES + TAIL_BYTES;
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Region {
    Header { bit: usize },
    Data { frame: usize, bit: usize },
    Crc { frame: usize, bit: usize },
    Tail { bit: usize },
}

fn locate(stream_bit: usize) -> Region {
    use layout::*;
    let header_bits = HEADER_BYTES * 8;
    let frames_end = header_bits + FRAMES * FRAME_BYTES * 8;
    if stream_bit < header_bits {
        return Region::Header { bit: stream_bit };
    }
    if stream_bit >= frames_end {
        return Region::Tail {
            bit: stream_bit - frames_end,
        };
    }
    let rel = stream_bit - header_bits;
    let frame = rel / (FRAME_BYTES * 8);
    let bit = rel % (FRAME_BYTES * 8);
    match bit.checked_sub(FRAME_DATA_BYTES * 8) {
        Some(crc_bit) => Region::Crc {
            frame,
            bit: crc_bit,
        },
        None => Region::Data { frame, bit },
    }
}

impl std::fmt::Display for Region {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Region::Header { bit } => write!(f, "hdr+{bit}"),
            Region::Data { frame, bit } => write!(f, "f{frame}:{bit}"),
            Region::Crc { frame, bit } => write!(f, "f{frame}:crc{bit}"),
            Region::Tail { bit } => write!(f, "tail+{bit}"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct BitDiff {
    stream_bit: usize,
    a: bool,
}

fn diff(a: &[u8], b: &[u8]) -> Vec<BitDiff> {
    a.iter()
        .zip(b)
        .enumerate()
        .filter(|(_, (x, y))| x != y)
        .flat_map(|(byte, (x, y))| {
            (0..8)
                .filter(move |bit| (x ^ y) >> bit & 1 == 1)
                .map(move |bit| BitDiff {
                    stream_bit: byte * 8 + bit,
                    a: x >> bit & 1 == 1,
                })
        })
        .collect()
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let no_crc = match args.iter().position(|a| a == "--no-crc") {
        Some(i) => {
            args.remove(i);
            true
        }
        None => false,
    };
    let [a_path, b_path] = args.as_slice() else {
        eprintln!("usage: rbfdiff [--no-crc] <a.rbf> <b.rbf>");
        return ExitCode::from(2);
    };
    let read = |p: &str| -> Result<Vec<u8>, String> {
        let data = std::fs::read(p).map_err(|e| format!("{p}: {e}"))?;
        if data.len() != layout::TOTAL_BYTES {
            return Err(format!(
                "{p}: {} bytes, an uncompressed EP2C35 RBF is {}",
                data.len(),
                layout::TOTAL_BYTES
            ));
        }
        Ok(data)
    };
    let (a, b) = match (read(a_path), read(b_path)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("rbfdiff: {e}");
            return ExitCode::from(2);
        }
    };
    let mut shown = 0;
    for d in diff(&a, &b) {
        let region = locate(d.stream_bit);
        if no_crc && matches!(region, Region::Crc { .. }) {
            continue;
        }
        shown += 1;
        println!(
            "{} {} {}->{}",
            d.stream_bit,
            region,
            u8::from(d.a),
            u8::from(!d.a)
        );
    }
    eprintln!("{shown} bits differ");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_lsb_first_stream_bits() {
        let d = diff(&[0x00, 0b1000_0001], &[0x00, 0b0000_0000]);
        assert_eq!(
            d,
            vec![
                BitDiff {
                    stream_bit: 8,
                    a: true
                },
                BitDiff {
                    stream_bit: 15,
                    a: true
                },
            ]
        );
    }

    #[test]
    fn total_matches_observed_file_size() {
        assert_eq!(layout::TOTAL_BYTES, 856_076);
    }

    #[test]
    fn locates_frame_boundaries() {
        let h = layout::HEADER_BYTES * 8;
        assert_eq!(locate(h - 1), Region::Header { bit: h - 1 });
        assert_eq!(locate(h), Region::Data { frame: 0, bit: 0 });
        assert_eq!(locate(h + 312 * 8), Region::Crc { frame: 0, bit: 0 });
        assert_eq!(locate(h + 314 * 8), Region::Data { frame: 1, bit: 0 });
        assert_eq!(locate(856_076 * 8 - 1), Region::Tail { bit: 36 * 8 - 1 });
    }
}
