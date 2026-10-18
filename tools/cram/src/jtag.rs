//! JTAG configuration of the EP2C35 as an SVF, the sequence Quartus 13.0sp1's
//! `quartus_cpf -c ... .svf` writes (README "JTAG configuration").
//!
//! The JTAG stream is not the RBF: the passive-serial preamble (0x000-0x04b:
//! `ff` padding, option words, `00` + 32 × `ff`) is replaced by 54 `ff`
//! bytes, and 4 `00` bytes follow the postamble. Shifting the raw RBF loads
//! "fine" and leaves the device unconfigured, which is what openFPGALoader
//! 1.1.1 does for Cyclone II `.rbf` files.

use crate::rbf::Rbf;
use std::fmt::Write;

const PS_PREAMBLE_BYTES: usize = 0x4c;
const JTAG_LEAD_FF: usize = 54;
const JTAG_TRAIL_00: usize = 4;

const IR_BITS: usize = 10;
const PROGRAM: u16 = 0x002;
const STARTUP: u16 = 0x003;
const CHECK_STATUS: u16 = 0x004;
const BYPASS: u16 = 0x3ff;

/// CHECK_STATUS shifts the 1449-bit boundary-scan register; bit 562 is
/// CONF_DONE, so a failed load fails the SVF instead of passing silently.
const STATUS_BITS: usize = 1449;
const CONF_DONE_BIT: usize = 562;

/// Idle waits, in TCKs. Quartus asks for 6000/30 at 6 MHz; these give the
/// same wall time at the USB-Blaster's 24 MHz.
const SETTLE_TCK: u32 = 24_000;
const SHORT_TCK: u32 = 120;
const INIT_TCK: u32 = 512;

pub fn stream(rbf: &Rbf) -> Vec<u8> {
    let body = &rbf.bytes()[PS_PREAMBLE_BYTES..];
    let mut s = Vec::with_capacity(JTAG_LEAD_FF + body.len() + JTAG_TRAIL_00);
    s.resize(JTAG_LEAD_FF, 0xff);
    s.extend_from_slice(body);
    s.resize(s.len() + JTAG_TRAIL_00, 0x00);
    s
}

/// SVF hex is the whole register as one number, last-shifted bit first; JTAG
/// shifts each byte LSB first, the order the stream is already in.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().rev().map(|b| format!("{b:02X}")).collect()
}

/// A `bits`-wide SVF value with only `bit` set (`None`: all zero), exactly
/// `bits / 4` digits rounded up.
fn register(bits: usize, bit: Option<usize>) -> String {
    let mut v = vec![0u8; bits.div_ceil(8)];
    if let Some(b) = bit {
        v[b / 8] |= 1 << (b % 8);
    }
    let h = hex(&v);
    h[h.len() - bits.div_ceil(4)..].to_string()
}

pub fn svf(rbf: &Rbf) -> String {
    let data = stream(rbf);
    let ir = |op: u16| format!("SIR {IR_BITS} TDI ({op:03X});\n");
    let mut s = String::from("! EP2C35 JTAG configuration, written by cram\nTRST ABSENT;\nENDDR IDLE;\nENDIR IRPAUSE;\nSTATE IDLE;\n");
    s += &ir(PROGRAM);
    writeln!(s, "RUNTEST IDLE {SETTLE_TCK} TCK ENDSTATE IDLE;").unwrap();
    writeln!(s, "SDR {} TDI ({});", data.len() * 8, hex(&data)).unwrap();
    s += &ir(CHECK_STATUS);
    writeln!(s, "RUNTEST {SHORT_TCK} TCK;").unwrap();
    let done = register(STATUS_BITS, Some(CONF_DONE_BIT));
    let zero = register(STATUS_BITS, None);
    writeln!(s, "SDR {STATUS_BITS} TDI ({zero}) TDO ({done}) MASK ({done});").unwrap();
    s += &ir(STARTUP);
    writeln!(s, "RUNTEST {SHORT_TCK} TCK;\nRUNTEST {INIT_TCK} TCK;").unwrap();
    s += &ir(BYPASS);
    writeln!(s, "RUNTEST {SETTLE_TCK} TCK;\nSTATE IDLE;").unwrap();
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rbf::TOTAL_BYTES;

    #[test]
    fn stream_drops_ps_preamble() {
        let mut b = vec![0xaa; TOTAL_BYTES];
        b[PS_PREAMBLE_BYTES] = 0x5c;
        let s = stream(&Rbf::parse(b).unwrap());
        // 6,848,464 bits, the SDR length in Quartus's SVF.
        assert_eq!(s.len() * 8, 6_848_464);
        assert!(s[..JTAG_LEAD_FF].iter().all(|&x| x == 0xff));
        assert_eq!(s[JTAG_LEAD_FF], 0x5c);
        assert_eq!(&s[s.len() - 5..], &[0xaa, 0, 0, 0, 0]);
    }

    #[test]
    fn conf_done_mask() {
        // 1449 bits = 363 hex digits; bit 562 is hex digit 140 from the
        // right, value 4.
        let m = register(STATUS_BITS, Some(CONF_DONE_BIT));
        assert_eq!(m.len(), 363);
        assert_eq!(&m[m.len() - 141..m.len() - 140], "4");
    }
}
