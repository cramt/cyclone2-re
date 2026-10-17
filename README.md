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

### Oracle harness (phase 0)

- `tools/oracle.sh <dir>` compiles `<dir>/top.v` plus `<dir>/assignments.qsf`
  (`quartus_map`, `quartus_fit`, `quartus_asm`, then
  `quartus_cpf -c -o bitstream_compression=off top.sof top.rbf`). About 90 s
  per design; six run in parallel fine on 8 cores. It writes its own
  `.qsf`/`.qpf` because a chipsmith manifest can't carry location assignments
  or Verilog. `chipsmith build` itself also works (VHDL only).
- `tools/quartus.sh <tool> <args>` runs any Quartus tool through
  `chipsmith run` inside `unshare --user --map-current-user --net`, and
  refuses to start if any interface other than `lo` is visible. chipsmith's
  own bubblewrap sandbox nests inside it without changes. Both a full
  `chipsmith build` and the oracle flow complete in there. The isolation is
  real: a mistyped `chipsmith run` without `--backend quartus-ii-13` tried to
  download Quartus Prime 23.1 and failed with a connection error.
- Compression: Cyclone II does accept compressed bitstreams (`-o
  bitstream_compression=on` gives 248 KB instead of 856 KB, and flips bit 0 of
  header byte 0x11). `quartus_cpf -c` defaults to off; the oracle says so
  explicitly. `quartus_asm` has no bitstream options on the CLI, only
  `--set=<assignment=value>` and settings-file assignments.
- Back-annotation works headlessly through
  `quartus_sh --tcl_eval exec quartus_cdb top --back_annotate=lc|routing`
  (`just back-annotate <dir>`). `lc` writes `set_location_assignment
  LCCOMB_X64_Y19_N0 -to lut` into the `.qsf`. `routing` writes `top.rcf`
  with every routing resource per net, for example:

  ```
  signal_name = SW[3] {
  	CLK_BUFFER:X33Y0S0I0;
  	C16:X33Y1S0I3;
  	R24:X34Y3S0I0;
  	C16:X57Y4S0I0;
  	R4:X58Y19S0I16;
  	R4:X62Y19S0I12;
  	LOCAL_INTERCONNECT:X64Y19S0I25;
  	dest = ( lut, DATAD ), route_port = DATAB;
  }
  ```

  Feeding the `.rcf` back in (`ROUTING_BACK_ANNOTATION_FILE`) makes the
  fitter reproduce the routing exactly ("preserve 100.00 percent of routes").
- The oracle pins two settings that would otherwise add noise or risk.
  `USE_CHECKSUM_AS_USERCODE OFF` stops the header changing with every
  design. `RESERVE_ALL_UNUSED_PINS "AS INPUT TRI-STATED"` must use that
  spelling. The `_NO_OUTPUT_GND` variant that back-annotation writes is
  ignored (warning 169174), and unused pins then drive ground.

### Container format (phase 1)

Observed on every uncompressed EP2C35 RBF so far (30 designs):

| Bytes | Content |
|---|---|
| 0x000-0x00f | `ff` preamble |
| 0x010-0x02a | `6a df ff 7f 00 60 80 68 00` three times (option words; bit 0 of 0x11 is the compression flag) |
| 0x02b-0x04b | `00`, then 32 × `ff` |
| 0x04c-0x074 | 41 bytes, bits 0-2 only. Bit 2 of bytes 0x4d-0x6c is the 32-bit JTAG USERCODE, LSB first. By default that is the SOF checksum from the assembler report, so it changes with every design. The oracle pins it with `USE_CHECKSUM_AS_USERCODE OFF` |
| 0x075-0x076 | CRC-16/MODBUS (poly 0xA001 reflected, init 0xffff) of bytes 0x4d-0x74, little endian |
| 0x077-0x185 | 271 × `01` |
| 0x186-0xd0fe7 | 2725 frames × 314 bytes: 312 data bytes + 2 check bytes |
| 0xd0fe8-0xd100b | 36 × `ff` postamble |

- Total: **856,076 bytes = 6,848,608 bits.** The 6,858,656-bit figure
  quoted from the handbook (not checked against the PDF) is 1,256 bytes,
  exactly 4 frames, more than this. Frame data is
  2725 × 2496 = 6,801,600 bits.
- **Each frame has its own check word.** For frames 24-2724 it is the
  reflected CRC-16 (poly 0xA001, the ARC/MODBUS polynomial) of the 312 data
  bytes with an effective init of 0xfe54, stored little endian. That holds for
  all 2701 of those frames in two different designs. An all-zero frame's word
  is `ef 32`. Frames 0-23 don't fit this model at any fixed length or init.
  Their data looks different too (8-byte runs of a repeated byte, 96-byte
  period). Switching unused pins from "output driving ground" to "input
  tri-stated" changes 9,687 bits: all of frames 1-23, plus one or two frames
  in most 35-frame columns (for example 108 and 122). So frames 0-23 are most
  likely the left I/O column with their own check scheme. That is an
  inference, not tested pin by pin.
- Empty design (one output pin tied to 0) against one LUT driving that pin:
  145 non-CRC bits differ, 396 counting CRC words. That is LUT plus routing
  plus input-pin config plus USERCODE.

### LUT bits (phase 1)

A WYSIWYG `cycloneii_lcell_comb` (`tools/gen_lut.py`) at `LCCOMB_X64_Y19_N0`
with mask 0x6996, routing pinned by its own `.rcf`, then 16 builds each
flipping one mask bit. **Every flip changes exactly one data bit**, apart
from the USERCODE noise this run still had:

| logical mask bit | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| frame | 2679 | 2677 | 2679 | 2677 | 2679 | 2677 | 2679 | 2677 | 2678 | 2676 | 2678 | 2676 | 2678 | 2676 | 2678 | 2676 |
| frame bit | 827 | 827 | 835 | 835 | 843 | 843 | 851 | 851 | 827 | 827 | 835 | 835 | 843 | 843 | 851 | 851 |

- Bits are stored **inverted**: a mask bit of 1 is a 0 in the RBF.
- The fitter permutes LUT inputs. The `.rcf` shows logical a→C, b→A, c→D,
  d→B (`route_port`). Rewritten in physical input order, the table becomes
  regular: for physical index `p = A + 2B + 4C + 8D`, the bit sits in frame
  `2679 - B - 2C` at frame bit `827 + 8·A + 16·D`. The layout is a 4 × 4 block:
  four adjacent frames, one bit position (bit 3) in four consecutive bytes.
  The permutation reading of `route_port` is an inference that happens to
  make the table regular. It isn't independently confirmed.
- Valid LCCOMB names use even N only: `LCCOMB_X64_Y19_N1` is rejected as
  an illegal location. Odd N is presumably the LCFF.

Moving the LE by one step. Each position compiles mask 0x6996 (XOR4) and
0x9669 (XNOR4). Both are symmetric, so input permutation can't matter, and
every pair differed in exactly 16 data bits:

| location | frames | frame bits | step from X64_Y19_N0 |
|---|---|---|---|
| X64_Y19_N0 | 2676-2679 | 827, 835, 843, 851 | - |
| X64_Y19_N2 | 2676-2679 | 795, 803, 811, 819 | N+2: -32 bits (-4 bytes) |
| X64_Y20_N0 | 2676-2679 | 1387, 1395, 1403, 1411 | Y+1: +560 bits (+70 bytes) |
| X63_Y19_N0 | 2641-2644 | 827, 835, 843, 851 | X-1: -35 frames |

- **X is the frame axis.** One LAB column spans 35 frames. Frames run
  left to right, since X64 sits near the end of the stream.
- **Y and N are in-frame.** All 16 LUT bits use bit 3 of their byte. N+2
  moves 4 bytes and Y+1 moves 70 bytes. A frame has 312 bytes, so 70 bytes
  per row fits only about 4.5 rows. A guess for the bit-in-byte axis: it
  selects one of 8 vertical bands of about 4-5 rows, which makes
  8 × 4.5 ≈ 36 rows, close to the EP2C35 row count. That needs Y samples
  from far apart in the column to confirm or rule out.
- Each location has one sample per axis, so linearity is assumed, not
  shown. Routing was deterministic. Compiling at a fixed location without an
  `.rcf` gave the same data bits as the `.rcf`-pinned build.

Nothing from Quartus gets redistributed. The repo ships only the derived bit
database and our own tools.

## Phases

| # | Phase | Status |
|---|-------|--------|
| 0 | Sandboxed oracle: generated design → chipsmith → `.sof`/`.rbf`, no network | done: `tools/oracle.sh`, verified offline |
| 1 | Container format: SOF/RBF framing, header, CRC, frame layout | RBF layout and frame CRC known for frames 24+; frames 0-23 and the header option words not decoded; SOF not looked at |
| 2 | Tile grid: LAB, M4K, multiplier, IOE, PLL coordinates | device DB unpacked, not parsed |
| 3 | Logic: LUT truth tables, FF modes, carry chains | LUT bits and X/Y/N strides from 4 positions; FF/carry not started |
| 4 | Routing: local, R4/C4/R24/C16 muxes, global clocks | not started |
| 5 | Hard blocks: IOE standards, M4K, multipliers, PLLs | not started |
| 6 | Flow: yosys techmap, nextpnr arch, Rust bitstream packer | not started |
| 7 | Loader: open JTAG programming (openFPGALoader already speaks USB-Blaster) | not started |

## Prior art

- [Project Mistral](https://prjmistral.readthedocs.io/): Cyclone V, upstream in nextpnr
- [Cyclone_CRAM_Mapper](https://github.com/14sea/Cyclone_CRAM_Mapper): Cyclone IV EP4CE6, partial routing
- No public Cyclone II effort found as of 2026-10-17
