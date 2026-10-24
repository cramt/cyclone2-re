#!/usr/bin/env bash
# Build one M4K fuzz design end to end: generate, compile, back-annotate
# placement + routing, and write the post-fit netlist.
#
#   tools/m4k_fuzz.sh <out-dir> <json config>
#
# NO_BA=1 stops after the RBF (enough for content-layout designs, whose
# only use is their RAM content bits).
set -euo pipefail
root="$(dirname "$(realpath "$0")")/.."
dir=$1; shift
"$root/tools/gen_m4k.py" "$dir" "$@"
"$root/tools/oracle.sh" "$dir" > "$dir.log" 2>&1 || { echo "$dir: oracle failed"; tail -5 "$dir.log"; exit 1; }
if [[ "${NO_BA:-}" != 1 ]]; then
    (cd "$root" && just back-annotate "$dir" && just netlist "$dir") >> "$dir.log" 2>&1
fi
echo "$dir: ok"
