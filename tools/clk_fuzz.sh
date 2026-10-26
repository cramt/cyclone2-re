#!/usr/bin/env bash
# Build one clock or PLL fuzz design end to end: generate, compile, back-annotate
# placement + routing, and write the post-fit netlist.
#
#   tools/clk_fuzz.sh <out-dir> <json config>
#
set -euo pipefail
root="$(dirname "$(realpath "$0")")/.."
dir=$1; shift
"$root/tools/gen_clk.py" "$dir" "$@"
"$root/tools/oracle.sh" "$dir" > "$dir.log" 2>&1 || { echo "$dir: oracle failed"; tail -5 "$dir.log"; exit 1; }
(cd "$root" && just back-annotate "$dir" && just netlist "$dir") >> "$dir.log" 2>&1
echo "$dir: ok"
