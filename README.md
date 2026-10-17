# cyclone2-re

An open bitstream database and toolchain for the Altera Cyclone II, starting
with the EP2C35F672C6 on the Terasic DE2.

Quartus Prime dropped Cyclone II. The last release that supports it is
Quartus II 13.0sp1, from 2013. The goal is that a DE2 never needs Altera
software again: Verilog in, yosys and nextpnr in the middle, a `.rbf` out, and
an open JTAG loader.

Quartus 13.0sp1 is used only as an **oracle**. It compiles generated test
designs so their bitstreams can be diffed. It never runs as part of the
finished flow. It runs sandboxed with no network, and once the database is
complete it can be deleted.

## Method

The pair-diff fuzzing method used by Project IceStorm (iCE40), X-Ray
(Xilinx 7-series), Trellis (ECP5) and Mistral (Cyclone V): compile two designs
that differ in exactly one feature, and the bits that change are the bits for
that feature. Reverse engineering of the shipped Quartus device data with REA
fills in what fuzzing can't reach cheaply, mainly routing graph topology.

## Findings

- **`.ddb` device files are not encrypted.** Each is a small plaintext
  header followed by standard XZ streams (`tools/unxz_ddb.py`). Codename for
  the Cyclone II family is `magellan`; `magellan4` is the EP2C35. Unpacked
  EP2C35 data: `icname` 7.2 MB (every wire name, e.g. `H4R35C66RT[0]`),
  `routing` 28.5 MB (routing graph, mux inputs like `IP:LEIM*`), `asm` 75.6 MB
  (assembler data, likely the config-bit map). Unpack with
  `python3 tools/unxz_ddb.py <file> specimens/ddb`.

Nothing from Quartus gets redistributed. The repo ships only the derived bit
database and our own tools.

## Phases

| # | Phase | Status |
|---|-------|--------|
| 0 | Sandboxed oracle: generated design → chipsmith → `.sof`/`.rbf`, no network | not started |
| 1 | Container format: SOF/RBF framing, header, CRC, frame layout | not started |
| 2 | Tile grid: LAB, M4K, multiplier, IOE, PLL coordinates | device DB unpacked, not parsed |
| 3 | Logic: LUT truth tables, FF modes, carry chains | not started |
| 4 | Routing: local, R4/C4/R24/C16 muxes, global clocks | not started |
| 5 | Hard blocks: IOE standards, M4K, multipliers, PLLs | not started |
| 6 | Flow: yosys techmap, nextpnr arch, Rust bitstream packer | not started |
| 7 | Loader: open JTAG programming (openFPGALoader already speaks USB-Blaster) | not started |

## Prior art

- [Project Mistral](https://prjmistral.readthedocs.io/): Cyclone V, upstream in nextpnr
- [Cyclone_CRAM_Mapper](https://github.com/14sea/Cyclone_CRAM_Mapper): Cyclone IV EP4CE6, partial routing
- No public Cyclone II effort found as of 2026-10-17
