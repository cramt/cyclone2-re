#!/usr/bin/env bash
# Build one routing fuzz design end to end: generate, compile, back-annotate
# the routing to top.rcf, and diff its RBF against the empty baseline.
#
#   tools/route_fuzz.sh <out-dir> <seed> [chains] [luts per chain]
set -euo pipefail
root="$(dirname "$(realpath "$0")")/.."
dir=$1; shift
"$root/tools/gen_route.py" "$dir" "$@"
"$root/tools/oracle.sh" "$dir" > "$dir.log" 2>&1
(cd "$root" && just back-annotate "$dir") >> "$dir.log" 2>&1
"$root/tools/rbfdiff/target/release/rbfdiff" --no-crc \
    "$root/specimens/route/empty/build/output_files/top.rbf" \
    "$dir/build/output_files/top.rbf" > "$dir/diff.txt"
echo "$dir: $(tail -1 "$dir/diff.txt" | head -c0)$(grep -c . "$dir/diff.txt") diff lines"
