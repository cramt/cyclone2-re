#!/usr/bin/env bash
# Build one I/O-cell register control design end to end: generate, compile, back-annotate
# placement + routing, and write the post-fit netlist.
#
#   tools/ioce_fuzz.sh <out-dir> <json config>
#
set -euo pipefail
root="$(dirname "$(realpath "$0")")/.."
dir=$1; shift
"$root/tools/gen_ioce.py" "$dir" "$@"
"$root/tools/oracle.sh" "$dir" > "$dir.log" 2>&1 || { echo "$dir: oracle failed"; tail -5 "$dir.log"; exit 1; }
(cd "$root" && just back-annotate "$dir" && just netlist "$dir") >> "$dir.log" 2>&1
echo "$dir: ok"
