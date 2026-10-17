//! `DYGR_ROUTE_ASM_INFO_BODY` (ddb_<die>_asm.ddb): which configuration RAM
//! bits each routing connection sets.
//!
//! One `DYGR_ROUTE_ASM_NODE` per routing element, in routing-element order.
//! A node's bits are split into groups; `get_destination_bit_group(src,
//! dst)` (libddb_dygr.so @ 0xeca70) returns the index of `dst` in `src`'s
//! fanout list, so group k holds the bits that turn on edge k of the source
//! element. Groups at or past the template's `first_purely_redundant_edge`
//! are "exclusion groups" (`get_exclusion_bit_group` @ 0xec980).

use anyhow::{ensure, Result};

use crate::pdb::Cursor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsmBit {
    /// Flat CRAM address (`DYGR_MAX_CRAM_FLAT_ADDRESS` caps it at 2^31-1).
    pub flat_address: u32,
    pub use_encoded_setting: u8,
    pub is_encoded_bit_high: u8,
    pub is_strangely_encoded: u8,
    pub is_cff_bit: u8,
}

#[derive(Debug, Clone, Default)]
pub struct AsmNode {
    /// 1-based index into `bits` where each group starts; a group ends where
    /// the next begins (`DYGR_ROUTE_ASM_NODE::get_num_bits_in_group` @ 0xebdd0).
    pub start_bit_index: Vec<u16>,
    pub bits: Vec<AsmBit>,
}

impl AsmNode {
    /// Bits of group `g`. A start index of 0 marks a group with no bits
    /// (`get_single_bit` asserts it is nonzero before indexing).
    pub fn group(&self, g: usize) -> &[AsmBit] {
        let start = self.start_bit_index[g] as usize;
        if start == 0 {
            return &[];
        }
        let end = self.start_bit_index[g + 1..]
            .iter()
            .find(|&&s| s != 0)
            .map_or(self.bits.len(), |&s| s as usize - 1);
        &self.bits[start - 1..end]
    }
}

pub fn parse(body: &[u8]) -> Result<Vec<AsmNode>> {
    let mut c = Cursor::new(body);
    c.ptr()?.expect_new()?;
    let num_asm_nodes = c.i32()?; // xfr_int_delta against a zero base
    let n = c.i32()?;
    ensure!(n == num_asm_nodes, "m_num_asm_nodes {num_asm_nodes} != vector size {n}");

    let mut nodes = Vec::with_capacity(n as usize);
    let (mut ngroups, mut nbits) = (0u16, 0u16);
    // Class objects (nodes, bits) are deltas against the previously
    // serialized object of the same class, so the bit chain runs straight
    // through node boundaries. Primitive vector elements are deltas against
    // the previous element, and element 0 against element 0 of the same
    // vector in the previous node (0 if that vector was empty).
    let mut first_start = 0u16;
    let mut prev = AsmBit { flat_address: 0, use_encoded_setting: 0, is_encoded_bit_high: 0, is_strangely_encoded: 0, is_cff_bit: 0 };
    for i in 0..n {
        // Node scalars are deltas against the previous node...
        ngroups = ngroups.wrapping_add(c.u16()?);
        nbits = nbits.wrapping_add(c.u16()?);
        let k = c.i32()?;
        ensure!(k as u16 == ngroups, "node {i}: {k} group starts for {ngroups} groups");
        let mut start_bit_index = Vec::with_capacity(k as usize);
        let mut acc = first_start;
        for _ in 0..k {
            acc = acc.wrapping_add(c.u16()?);
            start_bit_index.push(acc);
        }
        first_start = start_bit_index.first().copied().unwrap_or(0);
        let k = c.i32()?;
        ensure!(k as u16 == nbits, "node {i}: {k} bits, header says {nbits}");
        let mut bits = Vec::with_capacity(k as usize);
        for _ in 0..k {
            let b = AsmBit {
                flat_address: prev.flat_address.wrapping_add(c.u32()?),
                use_encoded_setting: prev.use_encoded_setting.wrapping_add(c.u8()?),
                is_encoded_bit_high: prev.is_encoded_bit_high.wrapping_add(c.u8()?),
                is_strangely_encoded: prev.is_strangely_encoded.wrapping_add(c.u8()?),
                is_cff_bit: prev.is_cff_bit.wrapping_add(c.u8()?),
            };
            bits.push(b);
            prev = b;
        }
        nodes.push(AsmNode { start_bit_index, bits });
    }
    ensure!(c.is_empty(), "{} trailing bytes after ASM nodes", c.remaining());
    Ok(nodes)
}
