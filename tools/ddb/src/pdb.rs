//! PDB serialization primitives, as implemented by `PDB_SEGMENT_READER` in
//! libdb_pdb.so. Everything is little-endian and unaligned; there are no
//! type tags in the stream, so the reader must know the schema.

use anyhow::{bail, ensure, Result};

pub struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Cursor { buf, pos: 0 }
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(
            self.remaining() >= n,
            "read of {n} bytes at {:#x} runs past end ({:#x})",
            self.pos,
            self.buf.len()
        );
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.bytes(N)?.try_into().unwrap())
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.arr::<1>()?[0])
    }
    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }
    /// `xfr_bool`: one byte, nonzero is true.
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.arr()?))
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.arr()?))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.arr()?))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.arr()?))
    }

    /// `get_c_str` / `xfr_c_str_v`: i32 length that *includes* a NUL which
    /// is not stored; 0 encodes a null pointer.
    pub fn c_str(&mut self) -> Result<Option<String>> {
        let n = self.i32()?;
        if n == 0 {
            return Ok(None);
        }
        ensure!(n > 0, "negative c_str length {n} at {:#x}", self.pos - 4);
        let b = self.bytes(n as usize - 1)?;
        Ok(Some(String::from_utf8_lossy(b).into_owned()))
    }

    /// `std::string` members: i32 length, then exactly that many bytes
    /// (no NUL accounting, unlike `c_str`).
    pub fn std_string(&mut self) -> Result<String> {
        let n = self.i32()?;
        ensure!(n >= 0, "negative string length {n} at {:#x}", self.pos - 4);
        Ok(String::from_utf8_lossy(self.bytes(n as usize)?).into_owned())
    }

    /// asmdb's strings: i32 exact length (no NUL accounting), 0 = none.
    pub fn std_string_opt(&mut self) -> Result<Option<String>> {
        let s = self.std_string()?;
        Ok((!s.is_empty()).then_some(s))
    }

    pub fn ptr(&mut self) -> Result<Ptr> {
        Ok(Ptr::decode(self.i32()?))
    }
}

/// A serialized object pointer (`PDB_SEGMENT_READER::read_ptr` @ 0x17a80).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ptr {
    Null,
    /// Object body follows inline; its class is footer class id `class_id`.
    New { class_id: u16 },
    /// Back-reference: object id = (last id in this pointer table + rel) & 0x3fffffff.
    Ref { rel: i32 },
}

impl Ptr {
    pub fn decode(v: i32) -> Ptr {
        match v {
            0 => Ptr::Null,
            v if v < 0 => Ptr::New { class_id: (-v) as u16 },
            rel => Ptr::Ref { rel },
        }
    }
    pub fn expect_new(self) -> Result<u16> {
        match self {
            Ptr::New { class_id } => Ok(class_id),
            p => bail!("expected inline object, got {p:?}"),
        }
    }
}

/// One footer entry (`PDB_SEGMENT_READER::read_db_footer` @ 0xfbf0).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClassInfo {
    pub class_id: i16,
    /// `sizeof` the in-memory C++ object.
    pub class_size: i32,
    /// Schema hash; Quartus refuses the file if it differs from the hash
    /// compiled into the reader, so it pins the field layout.
    pub hash: u32,
    pub number_of_instances: i32,
    pub name: String,
    /// Present only when the writer had not "processed" the class.
    pub base_pointer_table_size: Option<i32>,
}

pub fn parse_class_table(footer: &[u8]) -> Result<Vec<ClassInfo>> {
    let mut c = Cursor::new(footer);
    let n = c.i16()?;
    let mut out = Vec::with_capacity(n.max(0) as usize);
    for _ in 0..n {
        let class_id = c.i16()?;
        let class_size = c.i32()?;
        let hash = c.u32()?;
        let number_of_instances = c.i32()?;
        let name = c.c_str()?.unwrap_or_default();
        let processed = c.bool()?;
        let base_pointer_table_size = if processed {
            None
        } else {
            let largest_public_id = c.i32()?;
            ensure!(largest_public_id == 0, "largest base public id {largest_public_id} != 0");
            Some(c.i32()?)
        };
        out.push(ClassInfo { class_id, class_size, hash, number_of_instances, name, base_pointer_table_size });
    }
    ensure!(c.is_empty(), "{} trailing footer bytes", c.remaining());
    Ok(out)
}
