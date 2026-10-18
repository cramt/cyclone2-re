#!/usr/bin/env bash
# Fully open flow, no Quartus: Verilog -> yosys -> nextpnr -> cram pack.
#
#   tools/open_build.sh <design-dir>
#
# <design-dir> holds top.v (module `top`) and pins.qsf (Quartus-style
# `set_location_assignment PIN_x -to port` lines). Writes
# <design-dir>/build/{top.json,routed.json,top.rbf}.
#
# Needs the chip database from `just chipdb` (build/chipdb/) and the
# devshell's yosys and nextpnr-himbaechel (NEXTPNR overrides the binary).
set -euo pipefail

root=$(dirname "$(dirname "$(realpath "$0")")")
design=$(realpath "$1")
out="$design/build"
mkdir -p "$out"
chipdb="$root/build/chipdb/chipdb-EP2C35F672C6.bin"
# Base image: generated from the device DB (`cram blank`) unless CRAM_BASE names an RBF.
blank="${CRAM_BASE:--}"
nextpnr="${NEXTPNR:-nextpnr-himbaechel}"
cram="$root/tools/cram/target/release/cram"
[[ -f "$chipdb" ]] || { echo "no chip database: run 'just chipdb' first" >&2; exit 1; }

cd "$root"
yosys -q -l "$out/yosys.log" -p "read_verilog $design/top.v; script synth/cyclone2.ys; write_json $out/top.json"
"$nextpnr" --device EP2C35F672C6 --chipdb "$chipdb" --json "$out/top.json" --write "$out/routed.json" \
    -o qsf="$design/pins.qsf" --log "$out/nextpnr.log" -q
"$cram" pack "$blank" "$out/routed.json" "$out/top.rbf"
echo "open-build: $out/top.rbf"
