#!/usr/bin/env bash
# Fully open flow, no Quartus: Verilog -> yosys -> nextpnr -> cram pack.
#
#   tools/open_build.sh <design-dir>
#
# <design-dir> holds Verilog (*.v, top module `top`) and optionally pins.qsf
# (Quartus-style `set_location_assignment PIN_x -to port` lines; default
# examples/de2.qsf, the DE2 board's pins). Writes
# <design-dir>/build/{top.json,routed.json,top.rbf}.
#
# Needs the devshell's yosys and nextpnr-himbaechel, which carries the chip
# database built from db/ (NEXTPNR overrides the binary).
# cram reads the repo's device database (db/); no Quartus files.
set -euo pipefail

root=$(dirname "$(dirname "$(realpath "$0")")")
design=$(realpath "$1")
out="$design/build"
mkdir -p "$out"
# Base image: generated from the device DB (`cram blank`) unless CRAM_BASE names an RBF.
blank="${CRAM_BASE:--}"
nextpnr="${NEXTPNR:-nextpnr-himbaechel}"
cram="$root/tools/cram/target/release/cram"

pins="$design/pins.qsf"
[[ -f "$pins" ]] || pins="$root/examples/de2.qsf"

cd "$root"
yosys -q -l "$out/yosys.log" -p "read_verilog $design/*.v; script synth/cyclone2.ys; write_json $out/top.json"
"$nextpnr" --device EP2C35F672C6 --json "$out/top.json" --write "$out/routed.json" \
    -o qsf="$pins" --log "$out/nextpnr.log" -q
"$cram" pack "$blank" "$out/routed.json" "$out/top.rbf"
echo "open-build: $out/top.rbf"
