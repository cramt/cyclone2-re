#!/usr/bin/env bash
# Build one multiplier fuzz design end to end: generate, compile, back-annotate
# placement + routing, and write the post-fit netlist.
#   tools/mult_fuzz.sh <out-dir> <json config>
set -euo pipefail
root="$(dirname "$(realpath "$0")")/.."
dir=$1; shift
"$root/tools/gen_mult.py" "$dir" "$@"
"$root/tools/oracle.sh" "$dir" > "$dir.log" 2>&1 || { echo "$dir: oracle failed"; tail -5 "$dir.log"; exit 1; }
if [[ "${NO_BA:-}" != 1 ]]; then
    (cd "$root" && just back-annotate "$dir" && just netlist "$dir") >> "$dir.log" 2>&1
fi
echo "$dir: ok"
