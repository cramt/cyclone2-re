#!/usr/bin/env bash
# Build one LE/LAB fuzz design end to end: generate, compile, back-annotate
# placement + routing, and write the post-fit Verilog netlist (build/top.vo
# under simulation/modelsim), which is where LUT masks and register wiring
# are read from.
#
#   tools/lab_fuzz.sh <out-dir> <seed> [labs]
set -euo pipefail
root="$(dirname "$(realpath "$0")")/.."
dir=$1; shift
"$root/tools/gen_lab.py" "$dir" "$@"
"$root/tools/oracle.sh" "$dir" > "$dir.log" 2>&1
(cd "$root" && just back-annotate "$dir" && just netlist "$dir") >> "$dir.log" 2>&1
echo "$dir: ok"
