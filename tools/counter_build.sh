#!/usr/bin/env bash
# Build one counter design end to end (generate, compile, back-annotate,
# post-fit netlist).
#
#   tools/counter_build.sh <out-dir> <gen_counter.py args...>
set -euo pipefail
root="$(dirname "$(realpath "$0")")/.."
dir=$1; shift
"$root/tools/gen_counter.py" "$dir" "$@"
"$root/tools/oracle.sh" "$dir" > "$dir.log" 2>&1
(cd "$root" && just back-annotate "$dir" && just netlist "$dir") >> "$dir.log" 2>&1
echo "$dir: ok"
