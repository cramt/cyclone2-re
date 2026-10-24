//! The JTAG and serial-flash blocks of a Quartus post-fit netlist and the
//! lines its rcf routes into them; chipsmith's `bitstream::hard` turns them
//! into bits.

use anyhow::{bail, ensure, Context, Result};

use chipsmith_cyclone2::bitstream::hard::{Asmi, Hard, HardInput, Jtag, Line};

use crate::rcf::Signal;
use crate::vo::Atom;

/// The rcf resource the input mux drives.
fn rcf_kind(pin: HardInput) -> &'static str {
    match pin {
        HardInput::Tdouser => "JTAG_TDOUSER_PIN",
        HardInput::Dclk => "ASMI_DCLKIN",
        HardInput::Sce => "ASMI_SCEIN",
        HardInput::Sdo => "ASMI_SDOIN",
    }
}

/// The hard blocks of a Quartus post-fit netlist and the lines its rcf
/// routes into them.
pub fn from_atoms(atoms: &[Atom], sigs: &[Signal]) -> Result<Hard> {
    let line = |pin: HardInput| -> Result<Option<Line>> {
        for s in sigs {
            for (i, r) in s.resources.iter().enumerate() {
                if r.kind != rcf_kind(pin) {
                    continue;
                }
                let p = s.parents[i].map(|p| &s.resources[p]).filter(|p| p.kind == "LOCAL_INTERCONNECT");
                let p = p.with_context(|| format!("{}: {} not fed by a local line", s.name, r.kind))?;
                ensure!(
                    (p.x as u32, p.y as u32) == pin.xy(),
                    "{}: {} fed from X{} Y{}",
                    s.name,
                    r.kind,
                    p.x,
                    p.y
                );
                return Line::new(p.index as u32).map(Some);
            }
        }
        Ok(None)
    };
    let mut h = Hard::default();
    for a in atoms {
        match a.kind.as_str() {
            "jtag" => {
                ensure!(h.jtag.is_none(), "two JTAG blocks");
                h.jtag = Some(Jtag { tdouser: line(HardInput::Tdouser)? });
            }
            "asmiblock" => {
                ensure!(h.asmi.is_none(), "two serial-flash blocks");
                ensure!(a.port("oe") == Some("gnd"), "{}: only oe tied low is modelled", a.name);
                let need = |pin| line(pin)?.with_context(|| format!("{}: {pin:?} not routed", a.name));
                h.asmi = Some(Asmi { dclk: need(HardInput::Dclk)?, sce: need(HardInput::Sce)?, sdo: need(HardInput::Sdo)? });
            }
            "crcblock" => bail!("{}: the CRC block isn't modelled", a.name),
            _ => {}
        }
    }
    Ok(h)
}
