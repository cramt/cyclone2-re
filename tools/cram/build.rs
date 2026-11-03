// Embeds the digest of cram's sources (src/source_digest.rs).
#[path = "src/source_digest.rs"]
mod source_digest;

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // Directories are scanned whole, so new files count too.
    for f in ["tools/cram/Cargo.toml", "tools/cram/Cargo.lock", "tools/cram/build.rs", "tools/cram/src", "tools/ddb/Cargo.toml", "tools/ddb/src"] {
        println!("cargo:rerun-if-changed={}", root.join(f).display());
    }
    let d = source_digest::digest(&root).expect("hashing cram's sources");
    println!("cargo:rustc-env=CRAM_SOURCE_DIGEST={d:016x}");
}
