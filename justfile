rbfdiff := "tools/rbfdiff/target/release/rbfdiff"

_default:
    @just --list

# Compile a test design dir (top.v + assignments.qsf) to an uncompressed RBF, offline
oracle dir:
    tools/oracle.sh {{dir}}

# Bit-level diff of two RBFs, annotated with header/frame/CRC position
diff a b *flags: build-tools
    {{rbfdiff}} {{flags}} {{a}} {{b}}

build-tools:
    cargo build --release --quiet --manifest-path tools/rbfdiff/Cargo.toml

test:
    cargo test --manifest-path tools/rbfdiff/Cargo.toml
    cargo test --manifest-path tools/cram/Cargo.toml

# Write <dir>/build/top.rcf (routing) and LC/pin locations into top.qsf
back-annotate dir:
    cd {{dir}}/build && {{justfile_directory()}}/tools/quartus.sh quartus_sh --tcl_eval exec quartus_cdb top --back_annotate=lc
    cd {{dir}}/build && {{justfile_directory()}}/tools/quartus.sh quartus_sh --tcl_eval exec quartus_cdb top --back_annotate=routing

# Phase 1 LUT experiment: one design per flipped mask bit, routing pinned by base.rcf
lut-bits out loc rcf:
    tools/gen_lut.py {{out}}/base 6996 {{loc}} {{rcf}}
    for i in $(seq 0 15); do tools/gen_lut.py {{out}}/flip$i $(printf %04x $((0x6996 ^ (1 << i)))) {{loc}} {{rcf}}; done
    ls -d {{out}}/base {{out}}/flip* | xargs -P 6 -n 1 tools/oracle.sh

# Routing fuzz: one design per seed (generate, compile, back-annotate, diff vs empty)
route-fuzz out +seeds:
    for s in {{seeds}}; do echo $s; done | xargs -P 6 -I{} tools/route_fuzz.sh {{out}}/r{} {} 6 6

# Predicted (routing + LUT + LE/IO mux) vs observed bits for fuzz design dirs
cram-check +dirs:
    cargo build --release --quiet --manifest-path tools/cram/Cargo.toml
    tools/cram/target/release/cram check specimens/route/empty/build/output_files/top.rbf {{dirs}}

# Post-fit Verilog netlist: <dir>/build/simulation/modelsim/top.vo
netlist dir:
    cd {{dir}}/build && {{justfile_directory()}}/tools/quartus.sh quartus_sh --tcl_eval exec quartus_eda top --simulation --tool=modelsim --format=verilog

# LE/LAB fuzz: one design per seed (generate, compile, back-annotate, netlist)
lab-fuzz out +seeds:
    for s in {{seeds}}; do echo $s; done | xargs -P 6 -I{} tools/lab_fuzz.sh {{out}}/l{} {} 6

# I/O fuzz: one design per seed for a gen_io.py campaign (roles, attr, std, reg, unused, mixu)
io-fuzz out campaign +seeds:
    for s in {{seeds}}; do echo $s; done | xargs -P 6 -I{} tools/io_fuzz.sh {{out}}/{{campaign}}{} {} {{campaign}}

# Assemble each design dir from the model alone and compare with Quartus's RBF
roundtrip +dirs:
    cargo build --release --quiet --manifest-path tools/cram/Cargo.toml
    tools/cram/target/release/cram roundtrip specimens/route/empty/build/output_files/top.rbf {{dirs}}
