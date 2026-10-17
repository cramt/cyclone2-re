//! `DYGR_ROUTE_ICNAME_INFO_BODY` (ddb_<die>_icname.ddb): interconnect names.
//!
//! The body is two `std::map<unsigned, std::string>` (ic_name, then
//! ic_sch_name), serialized by the map template at libddb_dygr.so 0x2073a0:
//! i32 count, then per entry an `xfr_int_delta` key (relative to the
//! previous key, first relative to 0) and an i32-length string. Keys are
//! routing global ids: bit 31 set, low bits index the routing element list.

use anyhow::{ensure, Result};

use crate::pdb::Cursor;

pub struct IcNames {
    pub names: Vec<(u32, String)>,
    /// Schematic-view name ("XQ4/Q4_H4R35C66RT[0]"), "-" when there is none.
    pub sch_names: Vec<(u32, String)>,
}

fn map(c: &mut Cursor) -> Result<Vec<(u32, String)>> {
    let n = c.i32()?;
    ensure!(n >= 0, "negative map size");
    let mut key = 0u32;
    (0..n)
        .map(|_| {
            key = key.wrapping_add(c.u32()?);
            Ok((key, c.std_string()?))
        })
        .collect()
}

pub fn parse(body: &[u8]) -> Result<IcNames> {
    let mut c = Cursor::new(body);
    c.ptr()?.expect_new()?;
    let names = map(&mut c)?;
    let sch_names = map(&mut c)?;
    ensure!(c.is_empty(), "{} trailing bytes", c.remaining());
    Ok(IcNames { names, sch_names })
}
