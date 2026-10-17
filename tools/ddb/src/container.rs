//! The `.ddb` outer container: a fixed header written by
//! `PDB_SEGMENT_WRITER`, a compressed body, and a compressed footer that
//! holds the class table. Field names are the ones Quartus itself uses in
//! `PDB_SEGMENT_READER::read_db_header` (libdb_pdb.so @ 0x101d0).

use anyhow::{bail, ensure, Context, Result};
use std::io::Read;

use crate::pdb::Cursor;

/// `m_compression_type`. Only written when `m_version_number > 13`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Compression {
    /// zlib header + deflate, terminated by a sync flush instead of a final
    /// block, so strict decoders reject it as truncated.
    Zlib,
    Xz,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Header {
    pub version_number: i32,
    pub compression_level: i16,
    pub compression: Compression,
    pub size_of_header: i16,
    pub end_of_header_pos: i16,
    /// Quartus build string, e.g. "Version 13.0.1 Build 232 ...".
    pub creator: String,
    pub size_of_body: u64,
    pub end_of_body_pos: u64,
    pub size_of_footer: u64,
    pub end_of_footer_pos: u64,
}

/// 0x1bad marks a file whose writer died before committing.
const MAGIC: i16 = 0x0bad;
const MAGIC_TRUNCATED: i16 = 0x1bad;

pub struct Ddb {
    pub header: Header,
    pub body: Vec<u8>,
    pub footer: Vec<u8>,
}

impl Ddb {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&raw)
    }

    pub fn parse(raw: &[u8]) -> Result<Self> {
        let header = parse_header(raw)?;
        let body_range = header.end_of_header_pos as usize..header.end_of_body_pos as usize;
        let footer_range = header.end_of_body_pos as usize..header.end_of_footer_pos as usize;
        ensure!(footer_range.end <= raw.len(), "footer runs past end of file");
        let body = decompress(header.compression, &raw[body_range], header.size_of_body)?;
        let footer = decompress(header.compression, &raw[footer_range], header.size_of_footer)?;
        Ok(Ddb { header, body, footer })
    }
}

fn parse_header(raw: &[u8]) -> Result<Header> {
    let mut c = Cursor::new(raw);
    let version_number = c.i32()?;
    let magic = c.i16()?;
    if magic == MAGIC_TRUNCATED {
        bail!("truncated database (header id 0x1bad)");
    }
    ensure!(magic == MAGIC, "bad header id {magic:#x}");
    let compression_level = c.i16()?;
    let compression = if version_number > 13 {
        match c.i16()? {
            0 => Compression::Zlib,
            1 => Compression::Xz,
            t => bail!("unknown compression type {t}"),
        }
    } else {
        Compression::Zlib
    };
    let size_of_header = c.i16()?;
    let end_of_header_pos = c.i16()?;
    let creator = c.c_str()?.unwrap_or_default();
    // Only version 17 carries 64-bit section bounds; every Cyclone II file is 17.
    ensure!(version_number == 0x11, "unsupported header version {version_number}");
    Ok(Header {
        version_number,
        compression_level,
        compression,
        size_of_header,
        end_of_header_pos,
        creator,
        size_of_body: c.u64()?,
        end_of_body_pos: c.u64()?,
        size_of_footer: c.u64()?,
        end_of_footer_pos: c.u64()?,
    })
}

fn decompress(kind: Compression, src: &[u8], expect: u64) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expect as usize);
    match kind {
        Compression::Xz => {
            lzma_rs::xz_decompress(&mut std::io::BufReader::new(src), &mut out)
                .map_err(|e| anyhow::anyhow!("xz: {e:?}"))?;
        }
        Compression::Zlib => {
            // No final block, so read_to_end reports UnexpectedEof once the
            // input is exhausted; everything decoded up to then is valid.
            let mut z = flate2::read::ZlibDecoder::new(src);
            match z.read_to_end(&mut out) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {}
                Err(e) => return Err(e).context("zlib"),
            }
        }
    }
    ensure!(out.len() as u64 == expect, "decompressed {} bytes, header says {expect}", out.len());
    Ok(out)
}
