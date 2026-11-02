//! A cram under a checkout's tools/cram/target runs only while that checkout
//! still has the sources it was built from (src/source_digest.rs).
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../src/source_digest.rs"]
#[allow(dead_code)]
mod source_digest;

/// A copy of the digest's inputs with the binary at
/// tools/cram/target/release/cram, as the scripts find it.
fn fake_checkout(name: &str) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = std::env::temp_dir().join(format!("cram-stale-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for f in source_digest::files(&src).unwrap() {
        std::fs::create_dir_all(root.join(&f).parent().unwrap()).unwrap();
        std::fs::copy(src.join(&f), root.join(&f)).unwrap();
    }
    let bin = root.join("tools/cram/target/release");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_cram"), bin.join("cram")).unwrap();
    root
}

fn run(root: &Path) -> std::process::Output {
    Command::new(root.join("tools/cram/target/release/cram")).args(["addr", "0"]).env_remove("CRAM_ALLOW_STALE").output().unwrap()
}

#[test]
fn runs_in_the_checkout_it_was_built_from() {
    let root = fake_checkout("same");
    let out = run(&root);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn refuses_once_the_sources_change() {
    let root = fake_checkout("changed");
    let lib = root.join("tools/ddb/src/lib.rs");
    let mut s = std::fs::read_to_string(&lib).unwrap();
    s.push_str("\n// a later commit\n");
    std::fs::write(&lib, s).unwrap();
    let out = run(&root);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("built from other sources"));
    std::fs::remove_dir_all(root).unwrap();
}
