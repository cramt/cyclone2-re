//! A digest of the sources cram is built from, so a binary can tell whether
//! the checkout it sits in still matches it. build.rs embeds it; main
//! recomputes it over the checkout at startup. Both include this file, so
//! it uses std alone.
//!
//! The binary at tools/cram/target/release/cram is what the scripts
//! (tools/validate.sh, open_build.sh, chipsmith's benches) run, and it has
//! been replaced by copies of older builds: sha256's Quartus validation on
//! 2026-11-01/02 used a cram from before de2-arstn's clear-inversion export,
//! and reported 236 LAB bits as chipsmith's fault.
use std::path::{Path, PathBuf};

/// The inputs, relative to the cyclone2-re root: both crates' manifests and
/// sources, cram's lockfile (it pins chipsmith-cyclone2) and build script.
/// db/ is data, read at use time from the checkout, so it isn't one.
pub fn files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = ["tools/cram/Cargo.toml", "tools/cram/Cargo.lock", "tools/cram/build.rs", "tools/ddb/Cargo.toml"]
        .iter()
        .map(PathBuf::from)
        .collect();
    for dir in ["tools/cram/src", "tools/ddb/src"] {
        walk(root, Path::new(dir), &mut out)?;
    }
    out.sort();
    Ok(out)
}

fn walk(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for e in std::fs::read_dir(root.join(rel))? {
        let e = e?;
        let rel = rel.join(e.file_name());
        if e.file_type()?.is_dir() {
            walk(root, &rel, out)?;
        } else if rel.extension().is_some_and(|x| x == "rs") {
            out.push(rel);
        }
    }
    Ok(())
}

/// FNV-1a (64-bit) over each input's path, length and bytes, in path order.
pub fn digest(root: &Path) -> std::io::Result<u64> {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    };
    for f in files(root)? {
        let data = std::fs::read(root.join(&f))?;
        eat(f.to_string_lossy().as_bytes());
        eat(&(data.len() as u64).to_le_bytes());
        eat(&data);
    }
    Ok(h)
}
