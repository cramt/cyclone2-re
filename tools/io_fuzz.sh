#!/usr/bin/env bash
# Build one I/O fuzz design end to end: generate, compile, back-annotate
# routing (top.rcf) and placement (top.qsf).
#
#   tools/io_fuzz.sh <out-dir> <seed> <campaign> [fraction]
set -euo pipefail
root="$(dirname "$(realpath "$0")")/.."
dir=$1; shift
"$root/tools/gen_io.py" "$dir" "$@"
"$root/tools/oracle.sh" "$dir" > "$dir.log" 2>&1 || { echo "$dir: oracle failed"; tail -5 "$dir.log"; exit 1; }
(cd "$root" && just back-annotate "$dir") >> "$dir.log" 2>&1
echo "$dir: ok"
