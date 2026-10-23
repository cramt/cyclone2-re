//! `DYGR_ROUTE_TIMING_INFO_BODY` (`ddb_<die>_timing.ddb`, libddb_dygr.so):
//! the physical parameters Quartus' delay calculator uses for every routing
//! element. Not delays: per element a wire length, resistance and
//! capacitance per micron, and the driver (an NLSPC switch model id); per
//! fanout load the position along the wire and the two SRAM indices of the
//! mux it enters. Delays come from feeding these through the NLSPC driver
//! models (`ddb_cycloneii-<corner>_nlspc*.ddb`); see docs/timing-model.md.
//!
//! Decoded here: everything up to and including the temperature info, which
//! is the per-element part. The deferred `DYGR_TIMING_BLOCK_TYPE` /
//! `DYGR_ROUTE_TIMING_BLOCK_NODE` objects (LAB-internal nodes such as `LL:5`
//! and `IP:12B`) follow and are not decoded.
//!
//! Format notes (checked against the PDB_ASCII_DUMP trace with `ddb
//! route-timing-trace` and verify/rtiming_trace_tokens.py):
//!
//! - Scalars in `DYGR_TIMING_NODE_SIMPLE_INFO` (length, counts) are i16
//!   deltas against the previous simple info, across both simple-info
//!   vectors; `driver_pos` an i32 delta against the previous driver info.
//!   `resistance` / `capacitance` are vectors of f32 (one per available
//!   temperature).
//! - The index vectors are i16 (`simple_info_indice`) or i32
//!   (`load_base_indice`) deltas against the previous element, element 0
//!   against 0.
//! - `DYGR_TIMING_NODE_LOAD_INFO`: two i8 SRAM indices, then
//!   `load_position` as an i16 delta.

use anyhow::{ensure, Result};

use crate::pdb::{Cursor, Ptr};

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Driver {
    /// Position of the driver along the wire, same unit as `length`.
    pub pos: i32,
    /// NLSPC switch model id (`t_nlspc_driver_model` index, inferred).
    pub kind: i32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SimpleInfo {
    /// Wire length; microns (inferred: R and C are given per micron).
    pub length: i16,
    pub num_avail_temperatures: i16,
    /// Per available temperature. Unit inferred as ohm/um.
    pub resistance: Vec<f32>,
    /// Per available temperature. Unit inferred as fF/um.
    pub capacitance: Vec<f32>,
    pub drivers: Vec<Driver>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Load {
    pub sram1: i8,
    pub sram2: i8,
    /// Where on the wire the load hangs, same unit as `length`.
    pub position: i16,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RouteTiming {
    /// Index into `simple_info` per routing element (same order as the
    /// routing graph's element list).
    pub node_simple_info: Vec<u16>,
    pub simple_info: Vec<SimpleInfo>,
    pub block_interface_simple_info: Vec<SimpleInfo>,
    pub block_interface_node_simple_info: Vec<u16>,
    /// Per routing element, the index of its first load in `loads`. Its
    /// loads are its fanout edges in order (inferred: the count isn't
    /// stored).
    pub node_load_base: Vec<u32>,
    pub loads: Vec<Load>,
    /// `m_timing_node_load_idriver_info`: 2 bits per load.
    pub load_idriver_bits: Vec<u8>,
}

struct Rd<'a> {
    c: Cursor<'a>,
    log: Option<&'a mut Vec<String>>,
    prev_si: [i16; 3],
    prev_driver_pos: i32,
}

impl<'a> Rd<'a> {
    fn emit(&mut self, s: impl FnOnce() -> String) {
        if let Some(l) = self.log.as_deref_mut() {
            l.push(s());
        }
    }
    fn i32(&mut self) -> Result<i32> {
        let v = self.c.i32()?;
        self.emit(|| format!("{v:08X}"));
        Ok(v)
    }
    fn i16(&mut self) -> Result<i16> {
        let v = self.c.i16()?;
        self.emit(|| format!("{v:04X}"));
        Ok(v)
    }
    fn i8(&mut self) -> Result<i8> {
        let v = self.c.i8()?;
        self.emit(|| format!("{v:02X}"));
        Ok(v)
    }
    fn count(&mut self) -> Result<usize> {
        let n = self.i32()?;
        ensure!((0..1 << 24).contains(&n), "route timing: bad count {n} at {:#x}", self.c.pos() - 4);
        Ok(n as usize)
    }
    fn delta16(&mut self, prev: i16) -> Result<i16> {
        let v = prev.wrapping_add(self.c.i16()?);
        self.emit(|| format!("{v:04X}"));
        Ok(v)
    }
    fn delta32(&mut self, prev: i32) -> Result<i32> {
        let v = prev.wrapping_add(self.c.i32()?);
        self.emit(|| format!("{v:08X}"));
        Ok(v)
    }
    fn f32s(&mut self) -> Result<Vec<f32>> {
        let n = self.count()?;
        (0..n)
            .map(|_| {
                let v = f32::from_le_bytes(self.c.bytes(4)?.try_into().unwrap());
                self.emit(|| fmt_e(v as f64));
                Ok(v)
            })
            .collect()
    }
    fn simple_info(&mut self) -> Result<SimpleInfo> {
        let length = self.delta16(self.prev_si[0])?;
        let num_drivers = self.delta16(self.prev_si[1])?;
        let num_avail_temperatures = self.delta16(self.prev_si[2])?;
        self.prev_si = [length, num_drivers, num_avail_temperatures];
        let resistance = self.f32s()?;
        let capacitance = self.f32s()?;
        let n = self.count()?;
        ensure!(n == num_drivers as usize, "route timing: {n} drivers, num_drivers {num_drivers}");
        let mut drivers = Vec::with_capacity(n);
        for _ in 0..n {
            self.prev_driver_pos = self.delta32(self.prev_driver_pos)?;
            let kind = self.i32()?;
            drivers.push(Driver { pos: self.prev_driver_pos, kind });
        }
        Ok(SimpleInfo { length, num_avail_temperatures, resistance, capacitance, drivers })
    }
    fn indices16(&mut self) -> Result<Vec<u16>> {
        let n = self.count()?;
        let mut prev = 0i16;
        (0..n)
            .map(|_| {
                prev = self.delta16(prev)?;
                Ok(prev as u16)
            })
            .collect()
    }
}

pub fn parse(body: &[u8], log: Option<&mut Vec<String>>) -> Result<RouteTiming> {
    let mut r = Rd { c: Cursor::new(body), log, prev_si: [0; 3], prev_driver_pos: 0 };
    ensure!(r.c.ptr()?.expect_new()? == 1, "route timing body doesn't start with its info body");
    r.emit(|| "FFFFFFFF".into());
    // Block types are deferred objects (new pointers), decoded elsewhere.
    for _ in 0..r.count()? {
        let p = r.i32()?;
        ensure!(matches!(Ptr::decode(p), Ptr::New { .. }), "route timing: block type pointer {p:#x}");
    }
    let n = r.count()?;
    let simple_info = (0..n).map(|_| r.simple_info()).collect::<Result<Vec<_>>>()?;
    let num_nodes = r.delta32(0)?;
    let node_simple_info = r.indices16()?;
    ensure!(node_simple_info.len() == num_nodes as usize, "route timing: {} indices for {num_nodes} nodes", node_simple_info.len());
    let n = r.count()?;
    let block_interface_simple_info = (0..n).map(|_| r.simple_info()).collect::<Result<Vec<_>>>()?;
    let num_bi = r.delta32(0)?;
    let block_interface_node_simple_info = r.indices16()?;
    ensure!(block_interface_node_simple_info.len() == num_bi as usize, "route timing: block interface index count");
    let n = r.count()?;
    let mut prev = 0;
    let mut node_load_base = Vec::with_capacity(n);
    for _ in 0..n {
        prev = r.delta32(prev)?;
        node_load_base.push(prev as u32);
    }
    let num_loads = r.delta32(0)?;
    let n = r.count()?;
    ensure!(n == num_loads as usize, "route timing: {n} loads, m_num_loads {num_loads}");
    let mut loads = Vec::with_capacity(n);
    let mut prev_pos = 0i16;
    for _ in 0..n {
        let sram1 = r.i8()?;
        let sram2 = r.i8()?;
        prev_pos = r.delta16(prev_pos)?;
        loads.push(Load { sram1, sram2, position: prev_pos });
    }
    // An embedded DYGR_PACKED_ARRAY, introduced by its class id as an i16.
    let _class = r.i16()?;
    let _is_simple_array = r.i8()?;
    let _num_bits = r.delta16(0)?;
    let _size = r.delta32(0)?;
    let size_in_bytes = r.delta32(0)?;
    let n = r.count()?;
    ensure!(n == size_in_bytes as usize, "route timing: packed array {n} != {size_in_bytes} bytes");
    let mut load_idriver_bits = Vec::with_capacity(n);
    for _ in 0..n {
        load_idriver_bits.push(r.i8()? as u8);
    }
    Ok(RouteTiming {
        node_simple_info,
        simple_info,
        block_interface_simple_info,
        block_interface_node_simple_info,
        node_load_base,
        loads,
        load_idriver_bits,
    })
}

/// C's `%E`, as the trace prints floats.
fn fmt_e(v: f64) -> String {
    let s = format!("{v:.6E}");
    let (m, e) = s.split_once('E').unwrap();
    let e: i32 = e.parse().unwrap();
    format!("{m}E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
}
