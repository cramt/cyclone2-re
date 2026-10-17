#!/usr/bin/env bash
# Build one routing fuzz design end to end: generate, compile, back-annotate
# the routing to top.rcf, and diff its RBF against the empty baseline.
#
#   tools/route_fuzz.sh <out-dir> <seed> [chains] [luts per chain]
set -euo pipefail
root="$(dirname "$(realpath "$0")")/.."
dir=$1; shift
empty="$root/specimens/route/empty"
# The baseline every diff is against: one output pin driven low, built with
# the same oracle settings as the fuzz designs.
if [[ ! -f "$empty/build/output_files/top.rbf" ]]; then
    mkdir -p "$empty"
    printf 'module top(output LEDR0);\n  assign LEDR0 = 1'"'"'b0;\nendmodule\n' > "$empty/top.v"
    echo 'set_location_assignment PIN_AE23 -to LEDR0' > "$empty/assignments.qsf"
    "$root/tools/oracle.sh" "$empty" > "$empty.log" 2>&1
fi
"$root/tools/gen_route.py" "$dir" "$@"
"$root/tools/oracle.sh" "$dir" > "$dir.log" 2>&1
(cd "$root" && just back-annotate "$dir") >> "$dir.log" 2>&1
"$root/tools/rbfdiff/target/release/rbfdiff" --no-crc \
    "$root/specimens/route/empty/build/output_files/top.rbf" \
    "$dir/build/output_files/top.rbf" > "$dir/diff.txt"
echo "$dir: $(grep -c . "$dir/diff.txt") diff lines"
