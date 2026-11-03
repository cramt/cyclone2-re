# `cram`, with the device database it reads at use time (db/) installed next
# to it: db::locate walks up from the executable, so $out/bin/cram finds
# $out/db without CRAM_DB.
{ lib, rustPlatform }:

let
  root = ../.;
in
rustPlatform.buildRustPackage {
  pname = "cram";
  version = "0.1.0";

  # The flake source is the git tree, so target/ (gitignored) is never in it.
  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions [
      ../tools/cram
      ../tools/ddb
      ../db
    ];
  };
  buildAndTestSubdir = "tools/cram";
  cargoRoot = "tools/cram";
  cargoLock = {
    lockFile = ../tools/cram/Cargo.lock;
    # The assembler, from chipsmith (its ADR-0003): a git dependency, so the
    # vendored source needs a hash. Update it with the rev in Cargo.toml: it
    # covers the whole chipsmith tree at that rev, so it changes with every
    # rev, even one that leaves chipsmith-cyclone2 alone (de3386a kept the
    # old hash, and master's cram failed to build until qor2).
    outputHashes."chipsmith-cyclone2-0.1.0" = "sha256-05fbBsjRBSS/rtZjeoQ2eQN1KXqzITCeVEXTGd1mnhM=";
  };

  postInstall = ''
    cp -r db $out/db
  '';

  meta.mainProgram = "cram";
}
