#!/usr/bin/env bash
# Have Quartus assemble exactly the placement and routing the open flow
# chose, and compare bitstreams.
#
#   tools/validate.sh <design-dir>
#
# After tools/open_build.sh: exports <design-dir>/build/routed.json as an
# atom netlist + location assignments + rcf (cram quartus-export) into
# <design-dir>/build/q, compiles that with the network-isolated oracle,
# back-annotates it, and compares Quartus' RBF with ours.
set -euo pipefail

root=$(dirname "$(dirname "$(realpath "$0")")")
design=$(realpath "$1")
out="$design/build"
cram="$root/tools/cram/target/release/cram"
q="$root/tools/quartus.sh"

"$cram" quartus-export "$out/routed.json" "$out/q"
if [[ -f "$design/top.sdc" ]]; then
    cp "$design/top.sdc" "$out/q/top.sdc"
    echo "set_global_assignment -name SDC_FILE ../top.sdc" >> "$out/q/assignments.qsf"
fi
"$root/tools/oracle.sh" "$out/q" > "$out/q/oracle.log" 2>&1 || { tail -30 "$out/q/oracle.log"; exit 1; }
grep -h "Router is attempting to preserve\|does not exist\|Critical Warning\|Error" "$out/q/build/fit.log" | sort | uniq -c | head -20 || true
(cd "$out/q/build" && "$q" quartus_sh --tcl_eval exec quartus_cdb top --back_annotate=routing > ba.log 2>&1 \
    && "$q" quartus_sh --tcl_eval exec quartus_eda top --simulation --tool=modelsim --format=verilog > eda.log 2>&1)
"$q" quartus_sta "$out/q/build/top" > "$out/q/sta.log" 2>&1 || true
grep -h "Fmax\|Worst-case\|Critical" "$out/q/sta.log" | head -8 || true
echo -n "validate: "
"$cram" compare "$out/top.rbf" "$out/q/build/output_files/top.rbf"
cmp -s "$out/top.rbf" "$out/q/build/output_files/top.rbf" && echo "validate: BYTE-IDENTICAL"
