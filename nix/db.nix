# The device database (db/: the EP2C35's configuration bits and the chip
# description its routing graph is built from) on its own. It's data that
# chipsmith reads (chipsmith ADR-0003), so taking it shouldn't mean building
# cram.
{ lib, runCommand }:

runCommand "cyclone2-db" {
  src = lib.fileset.toSource {
    root = ../.;
    fileset = ../db;
  };
} ''
  cp -r $src/db $out
''
