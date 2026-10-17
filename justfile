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

# Write <dir>/build/top.rcf (routing) and LC/pin locations into top.qsf
back-annotate dir:
    cd {{dir}}/build && {{justfile_directory()}}/tools/quartus.sh quartus_sh --tcl_eval exec quartus_cdb top --back_annotate=lc
    cd {{dir}}/build && {{justfile_directory()}}/tools/quartus.sh quartus_sh --tcl_eval exec quartus_cdb top --back_annotate=routing

# Phase 1 LUT experiment: one design per flipped mask bit, routing pinned by base.rcf
lut-bits out loc rcf:
    tools/gen_lut.py {{out}}/base 6996 {{loc}} {{rcf}}
    for i in $(seq 0 15); do tools/gen_lut.py {{out}}/flip$i $(printf %04x $((0x6996 ^ (1 << i)))) {{loc}} {{rcf}}; done
    ls -d {{out}}/base {{out}}/flip* | xargs -P 6 -n 1 tools/oracle.sh
