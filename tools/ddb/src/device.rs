//! One die's routing graph joined with its routing config bits, from
//! Quartus' files. The `Device` type is chipsmith's
//! (`chipsmith_cyclone2::bitstream::device`), which reads the same graph
//! from the shipped database.

use anyhow::{Context, Result};
use std::path::Path;

use crate::{asm, container::Ddb, routing};

pub use chipsmith_cyclone2::bitstream::device::{Device, Node};

pub fn load(devinfo: &Path, die: &str) -> Result<Device> {
    let open = |kind: &str| -> Result<Ddb> {
        let p = devinfo.join(format!("ddb_{die}_{kind}.ddb"));
        Ddb::open(&p).with_context(|| format!("opening {}", p.display()))
    };
    Device::new(routing::parse(&open("routing")?.body)?, asm::parse(&open("asm")?.body)?)
}
