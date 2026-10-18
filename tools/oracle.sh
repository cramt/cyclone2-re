#!/usr/bin/env bash
# Compile one test design with the Quartus oracle and leave an uncompressed
# .rbf next to it.
#
#   tools/oracle.sh <design-dir>
#
# <design-dir> holds top.v (top-level module `top`), or top.vqm (an atom
# netlist, which quartus_map takes as already mapped), and optionally
# assignments.qsf, whose lines are appended verbatim to the generated settings
# file (location assignments, LUT pinning, ...). Output lands in
# <design-dir>/build/output_files/top.{sof,rbf} plus the usual reports.
#
# We generate the .qsf ourselves rather than going through `chipsmith build`
# because a manifest can't carry location assignments or Verilog sources;
# `chipsmith run` (inside tools/quartus.sh) still does the NixOS/bubblewrap
# spawning.
set -euo pipefail

design=$(realpath "$1")

# Each step gets its own empty network namespace via tools/quartus.sh.
q="$(dirname "$(realpath "$0")")/quartus.sh"

build="$design/build"
rm -rf "$build"
mkdir -p "$build"
cd "$build"

cat > top.qpf <<'EOF'
QUARTUS_VERSION = "13.0sp1"
PROJECT_REVISION = "top"
EOF

{
    cat <<'EOF'
set_global_assignment -name FAMILY "Cyclone II"
set_global_assignment -name DEVICE EP2C35F672C6
set_global_assignment -name TOP_LEVEL_ENTITY top
set_global_assignment -name PROJECT_OUTPUT_DIRECTORY output_files
EOF
    if [[ -f "$design/top.vqm" ]]; then
        echo "set_global_assignment -name VQM_FILE ../top.vqm"
    else
        echo "set_global_assignment -name VERILOG_FILE ../top.v"
    fi
    cat <<'EOF'
set_global_assignment -name NUM_PARALLEL_PROCESSORS 1
# Not the _NO_OUTPUT_GND spelling that back-annotation writes: 13.0sp1 ignores
# that one and falls back to driving unused pins to ground (warning 169174).
set_global_assignment -name RESERVE_ALL_UNUSED_PINS "AS INPUT TRI-STATED"
set_global_assignment -name STRATIX_DEVICE_IO_STANDARD "3.3-V LVTTL"
# The default USERCODE is the design checksum, which lands in the RBF header
# and drags the header check word with it: ~20 noise bits in every diff.
set_global_assignment -name USE_CHECKSUM_AS_USERCODE OFF
set_global_assignment -name STRATIX_JTAG_USER_CODE FFFFFFFF
EOF
    if [[ -f "$design/assignments.qsf" ]]; then cat "$design/assignments.qsf"; fi
} > top.qsf

"$q" quartus_map top --read_settings_files=on > map.log 2>&1 || { tail -40 map.log; exit 1; }
"$q" quartus_fit top --read_settings_files=on > fit.log 2>&1 || { tail -40 fit.log; exit 1; }
"$q" quartus_asm top --read_settings_files=on > asm.log 2>&1 || { tail -40 asm.log; exit 1; }
# SOF -> raw binary. Cyclone II does accept compressed streams (on: 248 KB
# instead of 856 KB for an empty design), so say off explicitly even though
# it's cpf's default.
"$q" quartus_cpf -c -o bitstream_compression=off output_files/top.sof output_files/top.rbf > cpf.log 2>&1 || { tail -40 cpf.log; exit 1; }
echo "oracle: $build/output_files/top.rbf"
