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

### Device database

Format details, with every claim marked observed or inferred, are in
[docs/ddb-format.md](docs/ddb-format.md). The reader is `tools/ddb` (Rust).

- **`.ddb` files are Quartus "PDB" object serialization**, not encrypted.
  Each file is a 0x74-byte header, then a compressed body, then a
  compressed footer. The header holds the version, the compression type
  and the section bounds. The compression is xz, or for `asmdb`, `lei`,
  `pwr` and `blkrba` zlib without a final block. The footer is a class
  table that lists class id, size, schema hash and name. The body has no
  type tags. Values are little-endian. Most fields are deltas against the
  same field of the previous object. Pointers are new-object or relative
  back-references, and pointed-to object bodies are written at the end.
  All 180 `cycloneii/*.ddb` files parse (`ddb info`).
- **Quartus prints its own annotated trace of every database it reads.**
  Put `PDB_ASCII_DUMP=on` in `quartus.ini` (read by
  `PDB_SEGMENT_BASE::serialize_begin`). Quartus then prints every field
  with its C++ member name. This trace is what the decoders are checked
  against.
- **Routing graph decoded (EP2C35 = `magellan4`):** 376,788 routing
  elements, 3,104,196 fanout edges, 210,403 shared element templates. Each
  element has x/y, a template, and a fanout base. The template has type,
  z/index, length, direction, and fanout offsets relative to that base.
  Every edge targets a routing element. No edges are marked redundant.
  Fan-in (mux inputs) is the inverse of the fanout lists. The elements and
  templates match the Quartus trace byte for byte.
- **Wire names decoded:** `icname` is two `map<global id, string>`, the
  ic name and the schematic name. 149,468 named wires such as
  `H4R35C66RT[0]`, `V16_C1UP_R[1]` and `GCLK0CS49B`. Global id bit 31
  marks a routing element, and the low bits are the routing element index.
  The other 227,320 elements are probably block ports (`IP:LEIM*`,
  `LL:*`, `LO:*`; inferred).
  Their names live in the part of the routing body that isn't decoded yet.
- **`asm` maps every routing connection to configuration bits.** There is
  one ASM node per routing element. Bit group *k* of a node is the bit set
  that turns on fanout edge *k* of that element
  (`get_destination_bit_group`). Extra groups past the fanout count are
  "exclusion groups", whose meaning is unknown. 7,996,500 bits in total.
  Edges carry 2 bits (2,956,721 edges), 1 bit (114,656), 9 bits (13,824) or
  none (18,995). Each bit is a "flat CRAM address" (6,715 to 6,707,826)
  with encoding flags. About 1.5% of bits use the "encoded setting" flags.
  All 376,788 ASM nodes match the trace exactly.
- **The flat CRAM address maps to RBF frame/bit**, solved below in
  [CRAM addressing](#cram-addressing-phase-4).
- **`asmdb` decoded: the block configuration tables** (`tools/ddb/src/asmdb.rs`,
  details in [docs/ddb-format.md](docs/ddb-format.md#asmdb-asmdb_die-block-configuration)).
  529 named *arch groups* (`LE_LUT_INPUT`, `LAB_CLOCK_FAST_MUX`,
  `CLKBUF_CLK_INPUT_SELECT_CYCLONEII`, ...), 17,290 muxes listing their
  bits, and the base address of every block instance (2,076 LABs, M4K,
  multiplier, PLL). The group type names come from the enum string table
  in libdb_asmdb.so. The decode matches the Quartus trace exactly (all
  muxes, groups and block lists) and parses on all six dies. `bcm2bits`
  is unused for Cyclone II, because no `.bcm.xml` files exist for this
  family.
- **`pad` and `pkg` decoded: package pin → pad → IOE** (`tools/ddb/src/pins.rs`,
  `ddb pins <devinfo> magellan4 fbga672`). 672 balls, 487 with a pad.
  Pad number, bank, I/O type and (x, y, z) match every fit report we
  have (61,152 package-pin rows, 2,035 placed pins). Decoded values match
  the Quartus trace.
- Export: `cargo run --release --manifest-path tools/ddb/Cargo.toml --
  export ~/altera/13.0sp1/quartus/common/devinfo/cycloneii magellan4
  specimens/db/magellan4` writes `nodes.csv`, `edges.csv` (dst, src,
  src_edge, asm bits), `icname.csv` and `summary.json` in about 20 s.
  These files are Quartus-derived and stay in `specimens/`. The same
  decoder runs on all six Cyclone II dies, `magellan1` through `magellan6`,
  with 62,667 to 776,454 routing elements. Only magellan4 is checked
  against the trace.

### Timing model

Details, with every claim marked observed, inferred or unknown, are in
[docs/timing-model.md](docs/timing-model.md).

- **Cell delays are stored. Routing delays are computed.** The per-corner
  DMF (`ddb_magellan4-6.dmf.ddb` for the slow model, `-m` for the fast one)
  is a tree of integer picosecond values: LUT, carry, register tco/tsu/th,
  IOE, clock buffers and some clock-network hops. `ddb dmf` decodes it and
  matches the Quartus trace token for token. Every LE and register delay in
  the oracle design's `quartus_sta` reports is a DMF value: 53/53 distinct
  values (22,513 rows) slow, 47/47 (22,270 rows) fast.
- Routing elements have no stored delay. `ddb_magellan4_timing.ddb` gives
  each element a length, R/µm, C/µm, a driver model and its load
  positions. `ddb route-timing` decodes this, matching the trace. The
  fitter turns these into delays with the NLSPC driver models, which are
  not decoded yet. The fitter's per-type average estimates (`fdi`) are
  within about 30% of STA medians and can serve as a first router cost.

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
| 0x186-0xd0fe7 | 2725 raw frames × 314 bytes: 312 data bytes + 2 check bytes. Raw frames 0-23 are really 6 frames with each byte repeated 4 times |
| 0xd0fe8-0xd100b | 36 × `ff` postamble |

- Total: **856,076 bytes = 6,848,608 bits.** The 6,858,656-bit figure
  quoted from the handbook (not checked against the PDF) is 1,256 bytes,
  exactly 4 frames, more than this. Frame data is
  2725 × 2496 = 6,801,600 bits.
- **Each frame has its own check word**: the reflected CRC-16 (poly
  0xA001, the ARC/MODBUS polynomial) of the 312 data bytes with an
  effective init of 0xfe54, stored little endian. An all-zero frame's word
  is `ef 32`.
- **Raw frames 0-23 are 6 frames with every byte sent 4 times.** Read
  0x186-0x1d75 as 1,884 groups of 4 equal bytes; collapsing each group
  gives 6 frames of 312 data bytes plus a check word, and that check word
  is the same CRC as everywhere else. Observed on all 55 uncompressed
  RBFs we have (every group uniform, every check word right); `cram
  verify` recomputes all of them. So the stream holds 6 + 2701 = 2707
  frames. Flat CRAM addresses (2701 columns) never reach the 6
  replicated frames. They hold the asmdb's "CFF" bits (bit value
  `0xa0000000 | k`, 12,456 of them): IOE drive strength, pull-up, bus
  hold, input buffer select and pin mode, the dedicated clock inputs and
  the PLLs. The map from k to a replicated-frame bit is in [I/O
  pins](#io-pins-phase-5). The earlier "8-byte runs, 96-byte period" was
  this replication. Why the device wants these frames 4 times is open (a
  slower shift clock for that region is a guess).
- Empty design (one output pin tied to 0) against one LUT driving that pin:
  145 non-CRC bits differ, 396 counting CRC words. That is LUT plus routing
  plus input-pin config plus USERCODE.

### JTAG configuration (phase 7)

An RBF is a passive-serial image and **is not what JTAG wants**. Decoding
`quartus_cpf -c top.sof top.svf` for blinky:

- `SIR 002` (PROGRAM), then one `SDR 6848464`: **54 × `ff`, RBF bytes
  0x04c-end, 4 × `00`.** The passive-serial preamble 0x000-0x04b (`ff`
  padding, option words, `00` + 32 × `ff`) is dropped; everything from the
  USERCODE block on is shifted unchanged, LSB first. **[obs]**, one design;
  `cram svf` reproduces every scan vector of Quartus' SVF.
- `SIR 004` (CHECK_STATUS), `SDR 1449` expecting bit 562 high: CONF_DONE in
  the boundary-scan register. **[obs]** on hardware: a load with one broken
  frame check word reads it low.
- `SIR 003` (STARTUP), 512 idle TCKs, `SIR 3FF` (BYPASS).

openFPGALoader 1.1.1 shifts a raw `.rbf` (preamble included) and does not
check CONF_DONE, so it reports success and the device stays unconfigured.
Hence `just flash` goes through `cram svf`.

### JTAG and serial-flash blocks (phase 7)

A serial flash loader (Quartus' SFL, chipsmith's `flash --persist`) is a
design that bridges the JTAG cable to the configuration flash. It needs two
hard blocks, one of each per device. Observed on three Quartus designs
(a JTAG-to-EPCS bridge, the JTAG block with every output routed to a pin,
the serial-flash block driven from switches), all byte-identical in
`cram roundtrip` (chipsmith's `bitstream/hard.rs`, the Quartus side in
`tools/cram/src/quartus/hard.rs`):

- **Where.** Both sit in the non-LAB column X6: `cycloneii_jtag` at
  `JTAG_X1_Y19_N0` (routing elements at X6 Y21-22, z 0), `cycloneii_asmiblock`
  at `ASMIBLOCK_X1_Y19_N2` (X6 Y20, z 2). The CRC block is at X6 Y20-21 z 1,
  not modelled. **[obs]**
- **Outputs** are routing elements that fan out like an LE output (R4, C4
  and local lines of X6/X7): `JTAG_TMSUTAP_PIN` 361, `TCKUTAP` 362,
  `TDIUTAP` 363, `SHIFTUSER` 364, `CLKDRUSER` 365, `UPDATEUSER` 366,
  `RUNIDLEUSER` 367, `USR1USER` 368, `ASMI_DATAOUT` 474 (DATA0). Matched
  by each rcf chain's first hop. **[obs]**
- **Inputs** have no graph edges; like an LE input, the last hop is block
  configuration: asmdb block type 9 (`JTAG_PIA_TO_TDOUSER`,
  `JTAG_PIA_TO_TDOUTAP`), 15 (`SPI_PIA_TO_DCLKO`, `_SCE`, `_SDO`, `_SPIOE`)
  and 11 (`CRC_PIA_TO_*`), named by libdb_asmdb.so's translator table.
  Each is a two-level source mux over the 15 local lines of its tile with
  absolute addresses; source *s* is line *s* (lines 0, 1, 3, 8, 9 seen into
  the serial-flash block, 12 into TDOUSER). **[obs]**
- **Side effects.** Using the serial-flash block (oe tied low, so it drives
  the pins) also sets option bit 0 of header byte 0x60 and clears the
  pull-ups that unclaimed ASDO and nCSO get. Using only the JTAG block
  changes nothing outside its mux. **[obs]**
- A design instantiating `cycloneii_jtag` has to connect its tms/tck/tdi/tdo
  to ports named `altera_reserved_tms` etc. (quartus_map error 15276
  otherwise); they become the dedicated JTAG pins, with no user I/O bits.
  **[obs]**
- The open flow has them as `CY2_JTAG` and `CY2_ASMI` (`synth/cells_sim.v`,
  bels `JTAG` at X13 Y22 and `ASMI` at X13 Y20 in the chip database), the
  serial-flash block only with oe tied low. cram packs them and exports them
  to Quartus. chipsmith places and routes them; this repo's nextpnr uarch
  doesn't pack them yet.

On the DE2, through chipsmith's bridge (USER0, every Shift-DR one SPI
transaction): DATA0 shows up on TDO in the same TCK, so the silicon ID
(0x14, an EPCS16) is bit 32 onward of a `0xab` shift. **[obs]**

**The EPCS image** for an RBF is what `quartus_cpf` writes (.sof -> .pof
-> .rpd for an EPCS16): the RBF's bytes except bit 3 of option byte 0x11
(and its copies at 0x1a, 0x23), set in a passive-serial RBF and clear in
the active-serial image. **[obs]**, one design. In the flash each byte is
bit-reversed, the order the DE2's factory image is stored in. **[obs]**
`PULSE_NCONFIG` (IR 0x001) makes the device reconfigure from the EPCS as at
power-up: CONF_DONE high, the image's USERCODE, its LEDs. **[obs]** on the
DE2 (RUN/PROG in RUN).

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
  moves 4 bytes and Y+1 moves 70 bytes. Resolved in phase 4: the in-frame
  axis is CRAM rows, 70 per LAB row, laid out as 8 bit-in-byte lanes of
  311 rows. See [CRAM addressing](#cram-addressing-phase-4).
- Each location has one sample per axis, so linearity is assumed, not
  shown. Routing was deterministic. Compiling at a fixed location without an
  `.rcf` gave the same data bits as the `.rcf`-pinned build.

### CRAM addressing (phase 4)

Method: `tools/gen_route.py` makes designs of 6 chains × 6 WYSIWYG buffer
LUTs, each LUT pinned to a random LAB (`(* keep *)` wires add free-placed
buffers too). `tools/route_fuzz.sh` compiles each one, back-annotates the
routing to `top.rcf` and diffs the RBF against an empty baseline (one
output pin tied low, built with the same pinned USERCODE).
`tools/cram` (Rust) resolves every rcf resource to a routing element,
every consecutive pair to a fanout edge, and every edge to its `asm` bit
group, then maps flat addresses to RBF bits. Of r1-r25, 17 compiled; the
other 7 drew a LAB in the X1-6 Y19-22 hole, which has no LEs and which
the fitter rejects (the generator now skips it). r1 predates `keep` and
is left out. h1001-h1006 and g2001-g2006 came later, all compiled.

**The mapping, observed** (every predicted bit is a flipped bit, see the
numbers below):

```
column = flat % 2701            row = flat / 2701
frame  = 2724 - column          (frames 24..2724; frames 0-23 never appear)
rows 0..2175:   lane = row / 311, byte = 311 - row % 311
rows 2176..:    lane = 7,         byte = 311 - (row - 2176)
frame bit = byte * 8 + lane     (LSB first, the order rbfdiff prints)
```

- A frame is 8 "lanes" (bit position within the byte), each a run of
  CRAM rows laid out from the end of the frame backwards. Lane 6 holds
  one row fewer than the others (rows 2175 and 2176 are both observed,
  in lanes 6 and 7). Why is open.
- Routing bits are active high: 0 in the empty design, 1 when the edge
  is used. 0 polarity errors in 16,630 predicted routing bits.
- `rcf` resource kinds are routing element types: `LE_BUFFER` 45,
  `LOCAL_INTERCONNECT` 276, `R4` 264, `R24` 268, `C4` 269, `C16` 275,
  `CLK_BUFFER` 289, with `S` = z and `I` = index. With that table every
  consecutive pair of resources in every fuzz rcf is exactly one edge.
  `IO_DATAOUT` and LE inputs are not routing elements: the graph ends at
  `LOCAL_INTERCONNECT`.

**Tile layout in CRAM rows/frames, observed:**

- LAB row Y (1-35) owns 70 CRAM rows starting at `18 + 70·(35 − Y)`.
  Rows 0-17 are the top I/O row; rows 2468-2487 the bottom one. This is
  the phase-1 "70 bytes per Y": 70 rows, which wrap from one lane into the
  next. No band structure is needed.
- In a LAB, LEs 0-7 take 4 rows each counting up from the LAB's first
  row; LEs 8-15 are their mirror image about the LAB's last row; rows
  32-37 sit between.
- The first LUT frame of LAB column X is 78 + the widths of columns 1..X−1:
  35 frames per LAB column, 166 per M4K column (13, 26, 52), 27
  for the centre column (39, the multipliers), plus 4 unexplained frames before columns 16
  and 50 (mirror images about the centre). Checked on all 60 LAB columns.
- LUT entry `p = A + 2B + 4C + 8D` of `LCCOMB_X_Y_N`: frame
  `lut_frame(X) + 3 − B − 2C`, LE row `3 − A − 2D` (mirrored for LEs
  8-15). Stored inverted, so an unused LUT is 0 (mask 0xffff). This
  matches the phase-1 table and all 918 LUTs of the fuzz designs.

**LE- and I/O-level muxes.** These started as learned tables and are
now read from the asmdb (see [LE, LAB and clock configuration](#le-lab-and-clock-configuration-phase-3)):

- Local line → LUT input: a two-level mux, 2 bits per input. The table
  learned for C/D from these designs (608 (line, port, LE) entries) is
  reproduced exactly by the asmdb `LE_LUT_INPUT` groups, which also cover
  A/B. The learned table is deleted.
- LE output enable: the "output enable" bit at `lut_frame + 5` is
  `LE_REG_OUT_YGR`, LUT-vs-register select for the output.
- Output pins: the learned bottom-row table (2 bits per pin selecting
  the local line into `IO_DATAOUT`) is replaced by the asmdb
  `IO_DATA_INPUT` groups, which cover every IOE on all four sides and
  every local line. See [I/O pins](#io-pins-phase-5).
- Everything else that was left (260 bits in frames 0-23 and 21 in the
  bottom I/O row for the 6/6 pin set) is pin configuration, now modelled
  from the device DB too.

**Validation** (`cram check`). Tables learned from r2, r3, r6-r15
("train"). Held out with the same pin set (6 in, 6 out): r16, r17, r19,
r22, r25 and h1001-h1006 from a later batch. Held out with a different
pin set (4 in, 4 out, 4 × 8 LUT chains): g2001-g2006.

| | train (12) | held out (11) | other pins (6) |
|---|---|---|---|
| routing bits predicted / observed | 6,949 / 6,949 | 6,272 / 6,272 | 3,409 / 3,409 |
| LUT bits predicted / observed | 5,184 / 5,184 | 4,752 / 4,752 | 2,496 / 2,496 |
| LE + I/O mux bits predicted / observed | 2,088 / 2,088 | 1,914 / 1,914 | 982 / 982 |
| residual: frames 0-23 (pin I/O config) | 3,120 | 2,860 | 1,116 |
| residual: top/bottom I/O rows | 252 | 231 | 80 |
| residual anywhere else | 0 | 0 | 0 |

Every residual is I/O: 260 bits in frames 0-23 plus 21 in the bottom
row per design for the 6/6 pin set, constant across designs. One g2006
output used local line 2, which isn't in the learned I/O table.

**Round trip, observed:** `cram assemble` starts from the empty RBF, sets
routing + LUT + LE/I-O mux bits from the model, copies the pin-set I/O
config (bits shared by all training designs, plus frames 0-23 verbatim
with their check words, which we can't compute), and recomputes the frame
and header check words. **All 11 held-out same-pin-set designs come
out byte-identical to Quartus's RBF** (I/O config from the 12 training
designs). For the other pin set, with the I/O config learned
leave-one-out from the other five g designs, 5 of 6 are byte-identical;
g2006 differs in 2 data bits (its output on local line 2, missing from
the table; the two bits are where the table's line-mod-3 pattern puts
them) plus the check words of the 2 frames.

That was the state before the I/O model: the pin configuration and frames
0-23 were copied from training designs with the same pin set. With the
I/O model (below) `cram assemble` takes no training designs at all, and
all 29 of these designs (r2-r25, h1001-h1006, g2001-g2006, g2006
included) come out byte-identical. r1, the first-generation design without
a pinned USERCODE, differs only in its header and 6 bits of LUTs whose
masks it can't name.

### LE, LAB and clock configuration (phase 3)

Everything an LE, a LAB and a global clock control block configures is
named and placed by the asmdb. `cram lab-check` confirms the asmdb
`LE_LUT_FUNCTION` positions equal the fuzz-fit LUT model for all 531,456
LUT bits (2,076 LABs × 16 LEs × 16). A LAB block is 70 CRAM rows × 35
columns, based at `lab_row(y)` and frame `lut_frame(x) + 12`, so it spans
frames `lut_frame − 22 ..= lut_frame + 12`.

The design side comes from the post-fit netlist (`just netlist <dir>`,
`build/simulation/modelsim/top.vo`). It has LUT masks in physical input
order, `sum_lutc_input`, carry nets and register ports. The `.rcf` says
which LAB line or LE feedback (`LOCAL_LINE`, index = LE) feeds each LE
input, which LAB control line (`LAB_CONTROL_MUX` 0-3) carries each
ENA/ACLR/SCLR/SLOAD, and which row-clock buffer (`BLK_CLK_BUF` 0-5)
clocks the LAB. chipsmith's `bitstream/logic.rs` turns that into settings.

Where the setting is a mux input, its bits come from the asmdb maps. For
the input muxes, `aux` gives the source's first-level bit and
`next_mux_selection` its second-level bit. Sources 0-37 are local lines
and 38-53 are LE outputs. That covers LE inputs A-D, LAB control lines,
LAB clock (sources 4-9 = `BLK_CLK_BUF` 0-5), CE/sclr/sload/aclr fast
muxes, and clkctrl input select. Where the asmdb only names a bit, the
value was **read off the fuzz designs** (`tools/gen_lab.py`, 9 designs,
54 LABs, 6 control sets each):

| group | value (observed) |
|---|---|
| `LE_USE_CARRY` | 1 when LUT C comes from a real carry-in net |
| `LE_USE_REGISTER_FEEDBACK` | 1 when LUT C is the LE's own register, unrouted |
| `LE_REG_OUT_YGR[0..2]` | per output (LE_BUFFER n, n+1, LOCAL_LINE): 1 = LUT, 0 = register |
| `LE_CLOCK_SELECT` | LAB clock index (empty design: 1) |
| `LE_CLEAR_SELECT` | 0 = aclr line 0; registers without aclr read 1 |
| `LE_USE_SYNC_CLEAR` = `LE_USE_SYNC_LOAD` | one shared bit: register takes the LAB's sclr *and* sload |
| `LAB_INVERT` 0 / 2 | LAB clock 0 / 1 inverted |
| `LAB_ENABLE_CE` j, `LAB_INVERT` 1 / 3 | both drop to 0 when clock j has a (non-inverted) enable |
| `LAB_INVERT` 5 / 4 | sclr used, non-inverted ⇒ 1 / sload used, non-inverted ⇒ 0 |
| `LAB_ENABLE_SYNC_SIGNALS` | 3-bit code: none 101, sclr 001, sload 111, sload tied high 100 |
| `LAB_CLEAR_FAST_MUX` | encoded: setting s sets bit s−1; a global aclr goes on both lines, with `LAB_CLR_INVERT_PAIR` = 11 |
| `CLKBUF_CLK_INPUT_SELECT_CYCLONEII` | `CLKBUF_IN` index, LSB first (unused 111); `CLKBUF_ENABLE_MODE` = 1 |

Fitter choices that the bitstream doesn't record and the model has to
reproduce, observed:

- CE 1 only reaches control lines 0/1 and CE 0 only lines 2/3 (asmdb
  maps). An enabled clock domain therefore takes the LAB clock its
  enable's line dictates. Otherwise inverted clocks come first, then row
  clock buffers in the order 5, 0, 1, 2, 3, 4 **[inf]**. Lowest buffer
  first fit the 7 two-clock LABs of the LE fuzz designs; M4K design h2
  and clock design k2 put buffer 5 first. Neither the global clock index
  nor the post-fit netlist's order fits all of them, and a chipsmith
  design exported to Quartus (three of seven two-clock LABs against this
  rule) shows the choice isn't settled. chipsmith therefore keeps one
  clock per LAB (see [Global clocks from the core](#global-clocks-from-the-core-phase-4)).
- Register packing: a register beside an unrelated LUT is fed through
  sdata (= the LE's datac mux) with sload tied high (`sload(vcc)` in
  the netlist).
- Register-only LEs get a `~feeder` LUT.

**Global clocks** run through the routing graph: pin `CLK_BUFFER` (289)
→ `CLKBUF_IN` (513) | clkctrl | `CLKBUF_OUT` (515) → `GLOBAL_CLK_H` (292,
index = G) → `GLOBAL_CLK_V` (293, `GCLK<n>`) → `LAB_CLK` (294, `RCLK`)
→ `BLK_CLK_BUF` (560). The GCLK → RCLK edges use the asm "encoded"
bits (9 per edge), whose value is `is_encoded_bit_high`, not 1. The
only non-graph bits are the clkctrl input select and enable above.

**Validation.** `cram check` on the 9 LE/LAB fuzz designs plus the
26-bit counter: 23,299 logic and 11,852 routing bits predicted, all
right. No unexplained bit is left in any LAB, the clock network or the
clock controls. What remains is I/O (frames 0-23, I/O rows and columns).

Held out: counters (`tools/gen_counter.py`) that played no part in
fitting the rules. They vary width (20-32), placement (free and pinned,
chains crossing LAB rows), clock pin (N2 = CLOCK_50, D13 = CLOCK_27),
and enable + `!KEY[0]` sync clear. With the I/O pin-set copy, c20 and
c32p missed 2 bits (an LED output on a local line the learned I/O table
hadn't seen), and the 9 LE fuzz designs missed 17-20 I/O bits each
(switches the fitter promoted to a global clock or clear net). With the
first-principles I/O model all 8 counters and all 9 LE fuzz designs are
byte-identical, with no training designs. A promoted pin needs nothing
special: its input drives the clock control through the `IO_DATAIN`
path the rcf names, and that path's delay setting is what changed.
c26e found the one rule the fuzz designs had missed: an inverted sclr
clears `LAB_INVERT` 5. The model now tracks port inversion.

### Global clocks from the core (phase 4)

A clock control takes a core signal as well as a pin: rcf `LOCAL_INTERCONNECT`
(a LAB line of the clock control's tile) → `BLOCK_INPUT_MUX` (532) →
`CLKBUF_IN` 4. Both hops are graph edges without bits; the line is chosen
by the clock control's `CLKBUF_INTERFACE_COREIN` G source mux, and the
input select code is 4. The clock enable comes the same way: line →
`BLOCK_INPUT_MUX` → `CLKBUF_ENABLE` (512), the line chosen by
`CLKBUF_INTERFACE_ENOUT` G. Settings, observed:

| | `CLKBUF_ENABLE_MODE` | `CLKBUF_ENABLE_AND_INVERT` bit 0 |
|---|---|---|
| `ena_register_mode = "none"`, enable tied on | 1 | 1 |
| `"falling edge"` (the atom default), enable tied on | 0 | 1 |
| `"falling edge"`, enable from the core | 0 | 0 |

Every clock control has a core input (left and right side: z 5-8, top and
bottom z 4-7). Quartus promotes any high-fanout clock to one by itself:
a register output (a divided clock) or the JTAG block's TCK. The PLLs feed
clock controls through `CLKBUF_IN` 2 and 3 (select codes 2, 3).

Fuzz designs (`tools/gen_clk.py`, `just clk-fuzz specimens/clk
tools/clk_jobs.txt`; kinds `div`, `tck`, `ena`): fit on k1 (a divided
clock on G8), k2 (TCK on G0, with CLOCK_50 in the same LABs), k3, k4
(enable from a switch on a pin clock and on a divided clock). Held out:
k5, k6 (divided clocks placed on G6 and G13, registered enable mode) and
k8 (TCK, falling edge). All 8 byte-identical (`cram roundtrip`); k7 (an
unregistered enable from the core) compiled after the model and was
byte-identical first time.

### PLLs (phase 5)

The EP2C35's four PLLs, from Quartus designs (`cycloneii_pll` atoms in the
post-fit netlist carry every setting), chipsmith's `bitstream/pll.rs` and
this repo's `quartus/pll.rs`. **[obs]** unless marked.

- **Where.** PLL_1 bottom left (routing tile X0 Y1), PLL_2 top right (X65
  Y35), PLL_3 top left (X0 Y35), PLL_4 bottom right (X65 Y1). Each takes
  the dedicated clock pins of one side and drives that side's four clock
  controls: PLL_1 the left (CLK0-3; the DE2's CLOCK_50 on PIN_N2 is
  CLK0), PLL_2 the right (CLK4-7; EXT_CLOCK, PIN_P26, is CLK7), PLL_3 the
  top (CLK8-11; CLOCK_27, PIN_D13, is CLK11), PLL_4 the bottom (PLL_4 not
  seen in a design **[inf]**). The asmdb instances 0-3 (bases 9120, 0,
  6192, 2928) are PLL_1, PLL_3, PLL_4, PLL_2.
- **Routing.** Clock pin `CLK_BUFFER` → `CLKPIN_TO_PLLCLKIN_MUX` (520, in
  the clock control tile) → `PLL_CLKIN` (329); counter k →
  `PLL_BUFFER` k (69) → `CLKBUF_IN` 2/3 of the side's clock controls, and
  counter 2 also → the `PLL<n>_OUT` pin's clock control; lock →
  `PLL_LOCKED_PIN` (347) onto R24/C4 like an LE output; `areset` and
  `pfdena` from the PLL tile's LAB lines through two-level source muxes
  (`PLL_ARESET_INPUT`, `PLL_PFD_ENA_INPUT`, source = line). Physical
  output k is counter k whichever altpll port uses it.
- **Bits.** Asmdb block type 43. The counters, loop and lock settings are
  CFF bits (the replicated frames) at the instance's base; the input
  selects and control muxes are core bits, one asmdb path per PLL. Three
  CFF lanes start with a PLL (blocks 122, 258, 380): the IOE data had left
  those boundaries in gaps, and the PLLs' bits only land inside their
  frames when the lanes start there. Each feature is written as a code,
  LSB first in its asmdb mux order:

| feature | code |
|---|---|
| m and c0-c2 high, low counts | the count, 16 as 0 |
| counter mode (`*_MULT_FACTOR`) | even 0, odd 2, bypass 1 (counts 0); an unused counter 3, counts 15/15, preset 0 |
| `*_PRESET` | the `initial` count; `OUTPUT_COUNTER_PHASE`, `M_TAB_SELECT` the VCO tap (`ph`) |
| n (`N_MULT_FACTOR`, `N_DIVIDED_BY`) | n = 1: 1, 0; n = 2, 3, 4: 0 and 3, 1, 0 |
| `VCO_PHASE_CONTROL` | a bit per VCO tap any counter uses |
| `VCO_DIVIDE_BY_2` | the atom's `vco_post_scale` = 2 |
| `CHARGE_PUMP_CURRENT` | 53 µA: 4, 80 µA: 7 |
| `LOOP_FILTER_R`, `_C` | 1 kΩ: 0, 2.5 kΩ: 3; 3 pF: 0 |
| `SELECT_LOCK_WINDOW` (and `_FOR_FB`) | by m × post-scale: 3 at 4, 4 to 6, 7 to 8, 8 to 10, 11 to 16, 15 above |
| `USAGE_MODE`, `FEEDBACK_SELECT`, `FB_COMP_DELAY` | normal: 1, 1, 44; no compensation: 0, 0, 0 |
| `CLOCK_SELECT` 0 | one-hot: the clock pin's index on its side |
| `ENABLE_PLL`, `USE_ENABLE_PIN`, `GLOCK_COUNTER_ENABLE`, `LOCK_SEL_GATE_CONF_DONE`, `LOCK_MODE` | 1, 1, 1, 1, 3 in every design |

- **What the fitter picks.** The VCO runs at 500-1000 MHz, the phase
  detector at 10-402.5 MHz (the atoms' `vco_min/max`, `pfd_min/max`).
  altpll's choice of counters sits at the VCO nearest 800 MHz (50 → 100
  MHz: m 16, c 8; 75/25/40 MHz: m 12; 10 MHz: m 6 with the VCO post-scaled
  by 2). The loop settings depend on m × post-scale alone (sweep m =
  4-32, n = 1-4): 53 µA to 7, then 80 µA; 1 kΩ to 12, then 2.5 kΩ; 3 pF.
  A `cycloneii_pll` atom given only m, n and the counters gets the same
  loop settings (a1), so an exported netlist can carry its counters.
- **Fuzz designs** (`tools/gen_clk.py` kind `pll`: altpll or, with
  `atom`, the atom itself). Fit: p1-p11, p13, p14 (ratios, phase shift,
  duty cycle, c1 or c2 alone, no compensation, CLOCK_27 and EXT_CLOCK into
  PLL_3 and PLL_2, lock to a LED, areset, pfdena, 400 MHz) and the m/n
  sweep m4-m32, n2m10, n2m16, n3m24, n4m12, n4m32. Held out: p15 (10 MHz,
  post-scale 2), p16 (three outputs with phase and duty), a1. 58 of 59
  designs byte-identical in `cram roundtrip`; p12 (counter 2 also onto the
  `PLL1_OUT` pin) is not modelled. m = 2, 3 didn't compile (Quartus was
  killed by the host), so the lock window below m = 4 is unknown.

Not covered: LAB clocks from local routing rather than a global, two
different aclr signals in one LAB, inverted aclr, a second clock domain
sharing a buffer, `LE_USE_REG_CASCADE_ARM` (never seen changing),
multipliers, a PLL's external clock output, clock switchover, its
`clkena` outputs (M4K: [below](#m4k-ram-blocks-phase-5)). These are flagged as `unmodelled` rather than
guessed.

### I/O pins (phase 5)

Every pin's I/O element is configured from the device DB: no training
designs and no copied bits. chipsmith's `bitstream/io.rs`.

**Where the bits are (device DB, observed):**

- Package pin → pad → IOE: `ddb_magellan4fbga672_pkg` and
  `ddb_magellan4_pad` (`ddb pins`, docs/ddb-format.md). 459 user IOEs
  (239 row, 220 column) plus 16 dedicated clock inputs.
- IOE → features: the asmdb groups of block types 5 (row IOE), 6
  (column IOE) and 60/61 (clock input p/n). Path *i* is the *i*-th pad
  of that type in pad-id order. Each IOE owns 43 (row) or 42 (column)
  features: 64,792 core CRAM bits and 8,497 CFF bits over all IOEs.
- CFF bits are the replicated frames 0-23. Bit *k* is in block *k* / 24
  (one block per IOE); each run of blocks fills one bit-in-byte lane of
  the 6 frames, 311 usable bytes per frame. Eight `(lane, first block,
  offset)` triples, fit to the unused-pin pull-up design and the 16 clock
  pins (table in docs/ddb-format.md), place every IOE bit of every fuzz
  design.
- Output data and output enable come from the LAB-local line named in
  the rcf through the asmdb `IO_DATA_INPUT` / `IO_PIA_TO_OE` two-level
  muxes. This replaces the learned bottom-row table and covers all four
  sides and every line (c20, c32p and g2006 used a line the table lacked).
- Input pins into the core: rcf `IO_DATAIN` is routing element 301 and
  I/O row clocks `TB_IO_CLK` are 298, both plain graph edges.

**What the values are (learned from fuzz designs, observed):** assembly
writes every feature of every IOE. Features not listed are 0.

| state | features set |
|---|---|
| unused (input tri-stated) | `PIN_MODE` 10, `REG_FEEDS_PIN_OUT` 10, `SELECT_IOST` 110, `OE_INVERT` |
| unused, other modes | bus hold: + `USE_BUS_HOLD`; pull-up: + `USE_PULL_UP`; driving ground (= unspecified signal): drive 24 mA LVTTL, no `OE_INVERT` |
| any used pin | `PIN_MODE` 10 (00 with an OE register), both `CE_TO_*REG_TIE_OFF`, `OUTPUT_DUTY_CYCLE` 11 (not on outputs below 3.3 V) |
| input | `OE_INVERT`; `PAD_TO_CORE_DELAY`#*k* = 110 for each `IO_DATAIN` index *k* the rcf uses, 111 if it carries the input register; with an input register, a path carrying the pad itself stays 000 |
| output, bidir | drive code (table below), data mux; output only: `SELECT_IOST` 110 (input buffer off) |
| output enable | always (`oe` vcc): nothing; tri-state: `IO_PIA_TO_OE` mux, `OE_INVERT` if the netlist's `oe` is `!net`; never (`oe` gnd): `OE_INVERT`; open drain: `PIN_MODE` 11 |
| input buffer | 1.8 / 1.5 V: `SELECT_IOST` 010; 3.3 / 2.5 V: 000 |
| output tied to 0 / 1 | `DATA_INVERT` / nothing |
| pull-up, bus hold | `USE_PULL_UP`, `USE_BUS_HOLD` |
| I/O registers | `*REG_CLK_FAST_MUX` one-hot at row clock buffer + 1 (only buffers 4 and 5 seen, the rest inferred; the output and OE registers share `OUTREG_CLK_FAST_MUX`, which an OE register alone also sets), `USE_CLR_*REG` = 1 (no clear), `*REG_POWERS_UP_HIGH` for power-up 1, output register: `REG_FEEDS_PIN_OUT` 10, OE register: `PIN_MODE` 00 |
| ASDO, nCSO unclaimed | input, pull-up, input buffer at the bank's VCCIO |
| nCEO unclaimed | output, `DATA_INVERT`, bank's default drive (LVTTL code in an LVCMOS bank) |

Drive codes (`IO_SET_DRIVE_STRENGTH_ARMSTRONG`) depend on the standard,
not only the current: 3.3-V LVTTL 4-24 mA all seen; 3.3-V LVCMOS, 2.5,
1.8 and 1.5 V only at the default current the fitter picked (row 12, 8,
12, 6 mA; column 24, 16, 12, 8 mA). Other combinations are rejected, not
guessed. Slew rate has no feature at all on Cyclone II.

The design side comes from the post-fit netlist (`cycloneii_io`:
location, mode, I/O standard, current, registers), the qsf (pull-up, bus
hold, unused-pin mode) and the rcf (local lines, input paths, I/O
clocks). A bank's standard is that of its used pins.

**Fuzz designs** (`tools/gen_io.py`, `just io-fuzz`): every user pin
independently used with probability 1/3 as input, output or bidir, wired
pin to pin so the core holds only routing. Campaigns: `roles` (LVTTL),
`attr` (+ random drive, pull-up, bus hold), `std` (one standard per
bank), `reg` (I/O-cell registers on a clock pin), `unused` (each unused
mode), `mixu` (roles + random unused mode). The rules above came from
roles1-10, attr1-24, std1-12, reg1-5 and unused1-4.

**Validation, observed** (`cram roundtrip`, which assembles from the
model alone and compares with Quartus's RBF):

| designs | what varies | byte-identical |
|---|---|---|
| roles1-10, attr1-24, std1-12, reg1-5, unused1-4 (rules fit here) | | 55 / 55 |
| roles25-30, val101-112 | new random pin sets, LVTTL | 18 / 18 |
| vala201-206 | new pin sets + random drive / pull-up / bus hold | 6 / 6 |
| roles11-24 | pin sets of attr11-24, other attributes | 14 / 14 |
| std13-22 | new pin sets, random standard per bank | 10 / 10 |
| reg6-22 | new pin sets, I/O-cell registers | 17 / 17 |
| mixu301-308 | new pin sets + random unused-pin mode | 8 / 8 |
| de2 d1-d4 (`tools/gen_de2.py`) | the board's pins: CLOCK_50 (N2), SW[17:0], KEY[3:0], LEDR[17:0], LEDG[8:0], with LUTs and a counter | 4 / 4 |
| counters c20-c32p, cnt26 | CLOCK_50 / CLOCK_27, LEDR, SW, KEY with logic | 8 / 8 |
| LE fuzz l1-l9 | switches promoted to global clock / clear nets | 9 / 9 |
| routing fuzz r2-r25, h1001-h1006, g2001-g2006 | 6/6 and 4/4 pin sets, LUT chains | 29 / 29 |
| routing fuzz r1 | first-generation design, USERCODE not pinned | 0 / 1: header + 6 LUT bits, not I/O |

178 of 179 designs come out byte-identical with no training designs
from their pin set; the held-out ones use 472 of the 487 pads (all four
sides: 121 left, 123 right, 114 top, 114 bottom; the other 15 are JTAG and
configuration pins). Every check word, including the 6 replicated
frames', is recomputed. The one miss is not an I/O residual.

**Output enable and bidirectional pins** (`tools/gen_oe.py`, `just oe-fuzz`,
`tools/oe_fuzz.sh`). The roles campaign's bidirs had their OE routed but
their input side dead. These designs put every used pin into one of 21
shapes: tri-state outputs and bidirs with OE from a pin, an inverted pin,
a LUT or a fabric register; inverted data; open drain (`d ? 1'bz : 0`);
OE tied high or never driven; OE, output and input registers alone and
together, including ones that power up high (an inverter after the
register, or an init of 1); input sides read back through other pins.
Variants add random drive, pull-up and bus hold (`attr`) or a random
standard per bank (`std`). How the post-fit netlist says it, observed:

- A tri-state pin is a `cycloneii_io` whose `oe` is a net, `!net` when the
  enable is inverted (Quartus folds the inverter into the IOE). An
  `output` port stays in `output` mode (input buffer off) even when
  tri-stated; an `inout` port is `bidir`.
- Open drain is `open_drain_output = "true"` with `oe` vcc and the data on
  `datain`; no asmdb group is named open drain, it is `IO_PIN_MODE` bit 1.
  An inout that is never driven comes out the same way with data vcc.
- An enable that folds to 0 (a stuck register) is `oe` gnd: `OE_INVERT`,
  as on an input.
- An inverter after an I/O register becomes `*_power_up = "high"` with the
  inverted signal before it.

In the model (chipsmith's `bitstream/io.rs`) a used pin is `Dir::In(Input)`, `Dir::Out(Output)`
or `Dir::Bidir(Input, Output)`. An `Output` has data (`OutData::Line` with
its optional output register, or a constant) and an `OutputEnable`
(`Always`, `OpenDrain`, `Never`, or `Line` with inversion and its optional OE
register). The input register carries its own pad-to-core paths. So an OE
register can't exist without a routed enable, and a constant can't be
registered. The one cross-field rule, that the output and OE registers share
one clock, is checked.

| designs | what varies | byte-identical |
|---|---|---|
| oe1-3, oereg21/22/25, oeinv53/54 (rules fit here) | shapes above | 8 / 8 |
| oe4-12 | new pin sets, base shapes | 9 / 9 |
| oereg23/24/26, oeinv51/52/55/56 | new pin sets, register shapes | 7 / 7 |
| oeattr31-34 | + drive, pull-up, bus hold | 4 / 4 |
| oestd41-44 | + random standard per bank | 4 / 4 |

32 of 32, about 2,100 tri-state, bidir and open-drain pins on all four
sides. The 179 designs above are unchanged (178 of 179). These designs live
in `specimens/oe/` (Quartus output, not committed).

What is inferred rather than observed: the CFF lane boundaries inside
unused gaps of the block numbering, and which named setting each learned
encoded value is (delay codes). The pad-to-core rule for a pin whose
register and pad both feed the core (the pad's path stays 000) is
observed on 2 pins only. Not covered, so `cram` refuses rather than
guesses: LVDS and other differential or VREF standards, non-default
currents outside 3.3-V LVTTL, DDIO, I/O register clears and clock
enables, output and OE registers on different clocks (they share one
clock mux; not tried), programmable delays the fitter
only picks under timing pressure, and PCI clamp
(`IO_USE_PCI_DIODE_CLAMP`, never seen set).

### M4K RAM blocks (phase 5)

Every M4K a Quartus design uses is configured from the device DB:
chipsmith's `bitstream/m4k.rs`. Tags as in [docs/ddb-format.md](docs/ddb-format.md):
**[obs]** observed, **[inf]** inferred.

**Where they are (asmdb, [obs]):**

- 105 blocks, asmdb block type 39, one per LAB row in LAB-grid columns
  X13, X26 and X52 (Quartus `M4K_X52_Y2`). A block is 70 CRAM rows × 166
  frames based at `lab_row(y)` and its highest frame (641, 1231, 2268), so
  X13 is frames 476-641, X26 1066-1231, X52 2103-2268: the three 166-frame
  columns of the tile layout are all M4K. The 35 multipliers (block type
  41, 70 × 27) are in the 27-frame centre column X39, not with the M4Ks.
- 132 arch groups name everything an M4K configures: 88 input muxes
  (`RAM_MS_DATA_INPUT` 0-35, `_DATA_INPUT_B` 0-17, `_WRITE_ADDRESS_INPUT`,
  `_READ_ADDRESS_INPUT`, `_BYTE_ENABLE_INPUT(_B)`, `_ADDRESS_STALL_INPUT(_B)`,
  `RAM_PIA_TO_INPUT_YGR` 0-7), the control fast muxes (`RAM_CLOCK_`, `_CE_`,
  `_CLEAR_`, `_WE_`, `_RE_FAST_MUX`), mode bits, and
  `RAM_CONTENT_YGR_MEAB`, a 4,608-bit "RAM" mux that lists the memory
  bits. `DATA_INPUT` 18-35 have the same bit lists as `DATA_INPUT_B` 0-17
  and `BYTE_ENABLE_INPUT` 2-3 as `_B` 0-1: one physical pin each.
- Routing: the LAB lines at an M4K tile (rcf `LOCAL_INTERCONNECT`, index up
  to 80) and its data outputs (rcf `M4K_BUFFER`, element 278) are graph
  nodes, so `cram`'s routing covers them unchanged once 278 is in the rcf
  table. Each input mux is a two-level source mux over the lines (87
  sources, `aux`/`next_mux_selection` like `LE_LUT_INPUT`). Control signals
  either come from a row clock (`BLK_CLK_BUF` 0-5) or through one of 8
  control inputs (rcf `M4K_LEFT_CONTROL_INPUT` k = `RAM_PIA_TO_INPUT_YGR` k,
  fed from a line).

**Memory contents ([obs]):** found with 12 designs of a 512 × 8 RAM whose
initial word *a* has bit *b* set iff bit *k* of `8a + b` is set
(`"init": {"linear": k}`), then checked on every fuzz design's random
contents. The array is 128 rows × 36 columns; `RAM_CONTENT_YGR_MEAB` bit
`col × 128 + row`, stored as written (not inverted). A port of physical
width P = 36, 18 or 9 sees word *a*, column *s* at linear position
`a·P + s` of the 36-column rows; widths 1, 2 and 4 skip every ninth column
(`l = a·P + s`, position `l/8·9 + l%8`). Logical ×8, ×16 and ×32 ports are
physically ×9, ×18 and ×36. Which column holds which data bit is the
fitter's choice: the rcf's `section ram_sublocations` gives each output
bit's column ("slot"), e.g. a simple dual port with bit 1 in the ninth
column. Slots count in the narrower port's words; a wider port's bit *k*
sits in narrow word `k / narrow width`. The post-fit netlist's `mem_init1 .
mem_init0` hold the contents in the words of the port whose outputs the rcf
names (A for single port and true dual port, B for simple dual port).

**Physical pins ([obs]):** an address of *n* bits uses pins `12 − n ..= 11`
(MSB-aligned). Port A pins 0-3 are `DATA_INPUT` 12-15 (unused data pins
below ×18), pin 4 `WRITE_ADDRESS_INPUT` 11, pins 5-11 `WRITE_ADDRESS_INPUT`
0-6. Port B pins 0, 1 are `DATA_INPUT` 26, 35, pins 2-4
`READ_ADDRESS_INPUT` 9-11, pins 5-11 `READ_ADDRESS_INPUT` 0-6. Data in:
port A column *c* on `DATA_INPUT` *c*, port B column *c* on 18 + *c*.
A port A data bit in column *c* has byte enable `c / 9`.

**Settings ([obs], from 28 designs: `tools/gen_m4k.py`, configs in
`tools/m4k_jobs.txt`):**

| group | value |
|---|---|
| `RAM_ENABLE`, `RAM_ENABLE_CE_INPUT(_B)`, `_CE_OUTPUT(_B)` | 1 in every used block |
| `RAM_USE_EXTENDED_WRITE_PULSE_MAGELLAN` | 11, except a true dual port whose port A never writes (one design) |
| `RAM_DATA_WRITE_MODE(_B)` | one-hot physical width of port A (B): ×1 bit 0 … ×36 bit 5 |
| `RAM_USE_TRUE_DUAL_PORT_MODE` | single port (ROM included) and true dual port |
| `RAM_FLOW_THROUGH_MODE(_B)` | single port with a write enable, true dual port |
| `RAM_REG_DATA_OUT(_B)` | 1 = output unregistered; in simple dual port the read port uses the A group, a ×36 read both |
| `RAM_SELECT_CLK_DATA_OUT(_B)`, `RAM_USE_CLR_DATA_OUT`, `RAM_SELECT_CLR_DATA_OUT` | output register clock / clear in use / which clear |
| `RAM_SELECT_CLK_READ_ENABLE_READ_ADDRESS_B` | port B address clock (unused: 1) |
| `RAM_SELECT_CLK_BYTE_ENABLE_DATA_B` | port B input clock; 0 for a ×36 simple-dual write, which borrows port B's data registers |
| `RAM_WRITE_ENABLE_INVERT`, `RAM_READ_ENABLE_INVERT` | 1 for a non-inverted port A / port B write enable |
| `RAM_CE_INVERT` k | 1 for an inverted clock enable (one design, `!wren`) |
| `RAM_USE_PACKED_MODE` | single port ×32/×36 (below) |
| fast muxes | the asmdb map value of the source *is* the bit pattern: sources 0-7 control inputs, 8-13 row clocks (clock from row clock 5 = 130 = bits 1 and 7) |
| `RAM_USAGE_MODE` | never changes from the unused block's value |

How Quartus builds the less obvious configurations ([obs]):

- A single port ×32/×36 is "packed": two ×18 atoms in one block, port A
  on words 128-255 and port B on 0-127 of a 256 × 18 array, one atom's bits
  not contiguous (0-8 and 27-35). The rcf sublocation's middle field is the
  half.
- A ×36 ROM is a read-only port B (`RAM_DATA_WRITE_MODE_B`, read address
  pins).
- A simple dual port with two clocks becomes a true dual port with port A
  reading.
- RAMs deeper than a block put one atom in each block with LE address
  decoding and output muxes; the rcf then names output bits after the atom
  (`ram_block1a1~PORTADATAOUT3`) instead of per-bit slices (`q_a[3]`).
  Wider ones split the bits (`port_a_first_bit_number`).

Found along the way, outside the M4K: a register packed beside an LUT that
reads the register on datac while sdata takes the routed datac
(`LE_USE_REGISTER_FEEDBACK`); clock controls fed from the core
(`LOCAL_INTERCONNECT` → `BLOCK_INPUT_MUX` → `CLKBUF_IN` 4) through the
`CLKBUF_INTERFACE_COREIN` source mux of their G, and from a user pin
through the clock delay control (`CLKBUF_IN` 6, select code 5); rcf cell
names with `entity:` hierarchy parts. The 179 earlier designs still
roundtrip 178/179 (r1 as before), and the open-flow examples pack to the
same RBFs.

**Validation, observed** (`cram roundtrip -`):

| designs | what varies | byte-identical |
|---|---|---|
| fit: a0, s1-s36, s8r/e/c/n, s16b, d1-d36, d8r/k/o, d32x8, d8x32, b8, b18, b16x4, r8, r36 | one feature each: width 1-36, mode, output register, clock enable, clear, byte enables, read-during-write, mixed widths, ROM | 28 / 28 |
| held out h0-h13 (random configs, 1-2 blocks) | first pass: 3 of h0-h6; RAMs split over blocks were missing. After adding them (from h0, h1, h2, h5): M4K bits right in all 14 | 13 / 14 |
| held out v0-v12 (v2, v13 don't compile: dual-clock true dual port is unsupported in 13.0, an unused aclr) | M4K bits right in 12/12 on first sight; v5, v11 missed two clock-control bits (a core-fed global clear), now modelled from them | 12 / 12 |
| held out w0-w13 | first pass 13/14: w13 (256 × 36 over two blocks) had its two ×18 atoms' bits in the wrong order, the non-contiguous split packed blocks also show. Now modelled from w13 | 14 / 14 |
| held out x0-x13 (x9 doesn't compile: dual clock with a ×36 port) | first pass 11/12: x6 (registered ×36 read) sets both ports' output register groups, now modelled; x13, the same case, was run after the fix | 13 / 13 |

Over all 53 held-out designs that compile: 52 byte-identical; the M4K bits
were right the first time cram met them in 47 (misses: h0, h1, h2, h5,
w13, x6) and are right in all 53 now.

The h2 miss is an LE fitter choice, not M4K: which LAB clock a LAB with
two clocks and no enables gives each (`logic::clock_slots`, fit on 7 LABs,
says lowest row clock first; h2's LAB took row clock 5 before 2).

Not covered, so `cram` reports the block as unmodelled: address stall,
port B read enable in simple dual port, inverted write enables, clocks or
clears, packed true dual ports, more than two atoms in a block. Settings
no design produced (a true-dual port B output clear, clock 1 on an output
register, clock enable 1 on its own) follow the same rules but are
unverified. The open flow uses a subset: [M4K in the open
flow](#m4k-in-the-open-flow).

### Open flow (phase 6)

Verilog → yosys → nextpnr → `cram pack` → `top.rbf`, with no Quartus
program and no Quartus output involved:

```
nix develop                       # first time ~10 min: builds nextpnr with its chip database from db/
just open-build examples/blinky   # examples/blinky/build/top.rbf
```

No Quartus files are read at any step: everything comes from the device
database committed under `db/` (below).

**Where the code lives.** The assembler moved to
[chipsmith](https://github.com/A-H-Technology/chipsmith)'s
`chipsmith-cyclone2` crate, `crates/chipsmith-cyclone2/src/bitstream/`
(its ADR-0003; "chipsmith's `bitstream/...`" below): the physical netlist
and its bits, the nextpnr frontend, the base image, the RBF container, the
JTAG stream, and the device database's format and reader. `cram` depends
on that crate, so `cram pack`, `cram svf` and chipsmith's builds run one
implementation, and `roundtrip`, `check` and `validate` here guard it.
What stays here: the ddb decoders (which fill in chipsmith's device and
asmdb types), `cram db`, the chip database export, the Quartus frontend
(`tools/cram/src/quartus/`, `rcf.rs`, `vo.rs`, `route.rs`) and `cram
quartus-export`.

**Device database** (`db/EP2C35F672C6/`, `tools/cram/src/db.rs` and
chipsmith's `bitstream/db.rs`). What the
use-time commands (`cram pack`, `blank`, `chipdb`, `quartus-export`) need,
derived once from Quartus' devinfo by `just db` (`cram db db`, about 6
minutes) and committed, the way prjxray-db or Apicula ship theirs. Two
xz files:

| file | size | contents |
|---|---|---|
| `device.bin.xz` | 3.2 MB (41.7 MB raw) | magic `C2RE-DB`, format version, then a [postcard](https://postcard.jamesmunns.com/wire-format) encoding of chipsmith's `bitstream::db::Stored`: the routing graph (376,788 elements, 210,403 templates) and its 7,996,500 routing bits, stored by column as deltas; the asmdb block tables; the 475 bonded user pins of the F672 package with their IOE, bank and dual-purpose role |
| `chipdb.txt.xz` | 9.3 MB (125.1 MB raw) | `cram chipdb` output, the input of `nextpnr/cyclone2/gen/arch_gen.py` |

`cram db` writes `device.bin.xz`, reads it back (it must re-encode to the
same bytes), and exports `chipdb.txt.xz` from what it read back, so the
nextpnr generator input can't drift from the packer's database. Output is
deterministic (fixed-size xz blocks), so an unchanged device and unchanged
code reproduce the committed files byte for byte. Rerun `just db` after
changing anything chipsmith's `bitstream/db.rs` stores or `chipdb.rs`
exports, and bump its `VERSION` when a stored type changes; a reader of
another version refuses the file. cram finds the database through `CRAM_DB` (the
`db/` directory) if set, otherwise in the checkout it was built in (walking
up from the executable, then the source path cargo built it from). The
reverse-engineering commands (`check`, `assemble`, `roundtrip`, `routing`,
`lab-check`, `dump`, `io-bits`) still read Quartus' files directly
(`CRAM_DEVINFO`).

A design dir holds `*.v` (top module `top`) and optionally `pins.qsf`
(`set_location_assignment PIN_x -to port` lines; default
`examples/de2.qsf`, the DE2's clocks, switches, keys, LEDs and UART).
`just validate <dir>` then has the oracle assemble the same placement and
routing and compares (below).

**Physical netlist** (chipsmith's `bitstream/phys.rs`). What a placed and routed
design configures in device terms: LE configs (LUT mask over physical
inputs, input sources, carry, register and its LAB clock slot), LAB-wide
settings, clock controls, every used pin's IOE settings, M4K blocks (mode
and port widths, output registers, control sources, which LAB line feeds
each input pin, the 4,608 content bits), and routed nets as
routing-graph edges. A `Pip` can only be built from two nodes the graph
connects. `bits()` turns it into CRAM. Two frontends build it:
`tools/cram/src/quartus/` (post-fit netlist + rcf + qsf, what `cram
assemble` and `roundtrip` use; still 178/179 byte-identical) and
chipsmith's `bitstream/nextpnr.rs` (routed himbaechel JSON, `cram pack`).

**Base image without Quartus** (`cram blank`, chipsmith's `bitstream/blank.rs`).
`cram assemble` used to start from Quartus' empty-design RBF. The base is
now built from the container layout, the asmdb's 928 default bits, the IOE
model with every pin unused, and the values unused blocks hold, named by
asmdb feature: per LE `LE_CLOCK_SELECT` and `LE_USE_REG_CASCADE_ARM`, per
LAB 11 bits (no clock enables, sync code 101, idle fast muxes, `LAB_INVERT`
1/3/4), 25 M4K bits, 17 bits of PLL 2 and 3, clock-control select 111 and
`CLKBUF_ENABLE_AND_INVERT`, plus one bit per LAB (frame `lut_frame + 2`, LAB
row 36) and per LAB row of the clock column (frame 1663) that no asmdb
feature names. These values were read off Quartus' empty design by
attributing each set bit to its asmdb owner
(`tools/cram/examples/blank_probe.rs`) **[obs]**. Check: all 179 fuzz
designs assembled on this base instead of Quartus' are byte-identical
except r1, as before (`just roundtrip`). The header's 41-byte USERCODE block
has a fixed bits-0/1 pattern whose meaning is unknown; it is the same in
all 190 pinned-USERCODE RBFs.

**Chip database** (`tools/cram/src/chipdb.rs` → `db/EP2C35F672C6/chipdb.txt.xz`
→ `nextpnr/cyclone2/gen/arch_gen.py` → bbasm). nextpnr's CMake builds it
from `HIMBAECHEL_CYCLONE2_DB` (this repo's `db/`) like any himbaechel
uarch's database, with Python and the committed text only;
`nix/nextpnr-cyclone2.nix` does that, so the 242 MB binary lives in the
nix store, not in git. Each Quartus tile (x, y) is two nextpnr tiles: (2x, y) holds
the routing graph (one wire `G<node>` per element, an alias for every
foreign element that drives one, one pip per graph edge in its
destination's tile: 2,440 tile types, 2.29 M wires, 3,104,196 pips), and
(2x+1, y) the logic. All 2,076 LABs share one tile type: 16 LUT bels (z
2k) and register bels (z 2k+1), the LE input muxes from the asmdb
`LE_LUT_INPUT` source maps, 16 permutation pips per LE (any physical input
to any LUT input; carry LEs keep theirs), output select, LAB clock, the
four control lines (`LAB_PIA_TO_INPUT`) and which register controls each
reaches (CE/clear/sync-clear fast-mux maps), carry wires joined LE 15 →
LE 0 of the LAB below. I/O tiles get IOE bels (data from the IOE's
`IO_DATA_INPUT` lines, output enable `OE` from its `IO_PIA_TO_OE` lines,
pad-to-core onto both `IO_DATA_IN` paths or the clock pin's `CLK_BUFFER`;
dedicated clock inputs have no `OE`), and 16 clock-control bels sit at their
`CLKBUF_OUT`. The 105 M4K blocks share one tile type `M4K` (details in [M4K
in the open flow](#m4k-in-the-open-flow)). Measured: generating it takes 9.5 minutes and 2.6 GB peak
(himbaechel's python generator and bbasm);
the binary is 243 MB, and nextpnr-himbaechel needs about 330 MB for a
small design.

**nextpnr uarch** (`nextpnr/cyclone2/cyclone2.cc`, packaged by
`nix/nextpnr-cyclone2.nix` against nixpkgs' nextpnr 0.11.1 source; the
devshell's `nextpnr-himbaechel`). Cells come from `synth/cyclone2.ys`:
`LCELL_COMB` (LUT4 + carry), `LCELL_FF` (data, clock, enable, async clear,
sync clear gated by the enable), pin buffers (`CY2_IBUF`, `CY2_OBUF`, and
`CY2_IOBUF` for tri-state outputs and bidirs: yosys' `tribuf` turns
`oe ? d : 1'bz` into tri-state buffers and `iopadmap -toutpad/-tinoutpad`
merges them into it), M4K RAM blocks (`CY2_M4K`, [below](#m4k-in-the-open-flow));
the packer adds clock controls. The port's direction
decides an IOBUF's mode, as in Quartus: on an `output` port the input buffer
stays off, every bit of an `inout` port is a bidir. Inverters into `OE` and
`DATAOUT` fold into the IOE (`OE_INVERT`, `DATA_INVERT`), constants into
`OE_CONST`/`OUT_CONST`: an `inout` that is only read is a bidir that never
drives, which (as Quartus maps it, observed) is an open drain with data 1;
one driven unconditionally is a bidir with `oe` tied high. The maps tie every unused cell input (LUT inputs and CIN to 1,
enable to 1, clears to 0), the yosys convention, so the cells can have
plain simulation models. The packer folds constants into LUT masks as
Quartus does, except a 1 on DATAC, DATAD or CIN, which it only disconnects:
that is what an unconnected input reads, and folding it would rewrite the
carry half of the mask (COUT reads DATAD = 0) or LUT input C, which DATAC
shares with CIN. Rules it enforces, each a Quartus-legal LAB: at most two
clock/enable pairs and one enable, one async clear, one sync clear per LAB;
registers on the sync-load path and sync-cleared ones don't mix; a register
sits beside the LUT that drives it, or takes its data on SDATA (input C)
with the LAB's sload tied high; at most 34 signals from outside a LAB.
That last number is measured on picorv32: 30 leaves 32-bit adders
unplaceable, 34 routes in about 80 router2 iterations, 38 in 280, no limit
had not converged after 15 minutes. Timing: none (no delay model yet), so
placement and routing are not timing-driven.

**What Quartus does to an atom netlist, and so what the flow does too**
(observed by feeding our netlists back through the oracle):

- Carry chains are rewritten with the carry inverted after every odd chain
  position, the unused carry half of the last LE holds a copy of the sum,
  and an unused sum half is zeros. The packer emits that form.
- Every one-input LUT is a "redundant logic cell" and is deleted
  (`REMOVE_REDUNDANT_*` assignments don't stop it): buffers vanish,
  inverters fold into the loads (LUT masks, the IOE's `IO_DATA_INVERT`,
  the LAB's enable/sync-clear inversion). The packer folds them first. An
  inverter in front of a register is pushed back into the driving LUT, or
  becomes an inverted copy of it; a register fed by a register gets the
  sync-load path. Inverting register-to-register feeds (what a register
  init of 1 turns into) are still re-placed by Quartus.
- Registers that can't leave their power-up 0 are "stuck at GND" and
  removed with everything they make constant. yosys (`opt -sat` with
  explicit zero inits) and the packer (constant propagation through
  registers) remove the same ones (32 in picorv32).
- With a timing constraint the fitter chooses pad-to-core delays per
  destination; the export pins them at 6 (`PAD_TO_CORE_DELAY`, the model's
  110 code).
- A clock control with a tied-on `ena` clears `CLKBUF_ENABLE_AND_INVERT`.

**Validation** (`just validate <dir>`, `tools/validate.sh`). `cram
quartus-export` writes the routed design as an atom netlist (`top.vqm`,
which quartus_map takes as already mapped; atoms named after their output
net, which is what the rcf calls them), location assignments for every LE,
pin and clock control, and the routing as an rcf with physical
`route_port`s. The oracle compiles that offline; `ROUTING_BACK_ANNOTATION`
makes the fitter keep the routes. Results, observed:

| design | LEs | nets | open build | Quartus routes kept | RBF vs Quartus | setup slack @ 50 MHz (Quartus STA) |
|---|---|---|---|---|---|---|
| blinky (26-bit counter, CLOCK_50 → LEDR[3:0]) | 26 | 85 | 20 s | 100 % | byte-identical | 16.6 ns |
| lfsr (32-bit, seed from SW, KEY[0] load) | 81 | 219 | 15 s | 100 % | byte-identical | 8.6 ns |
| uart_tx (115200 8N1 on UART_TXD) | 69 | 217 | 19 s | 100 % | byte-identical | 13.6 ns |
| picorv32 (RV32I core + ROM, LEDs) | 1,039 | 1,687 | 115 s | 100 % | byte-identical | 0.43 ns |
| bidir (9 tri-state/bidir pins on GPIO_0 and SRAM_DQ: OE from a switch, inverted, a LUT, a register; open drain; read back to LEDs) | 29 | 169 | 89 s (host at load 50) | 100 % | byte-identical | 16.6 ns |
| ram (7 M4Ks: ×1 and ×16 single port, ×8, ×32, ×36 simple dual port, ×8 and ×36 ROMs) | 151 | 541 | 17 s | 100 % | byte-identical | 12.8 ns |
| ram_widths (10 M4Ks: ×2, ×4, ×9, ×18 single port, ×1, ×2, ×4, ×9, ×16, ×18 simple dual port, 4 with initial contents) | 78 | 576 | 372 s (host loaded, mostly waiting) | 100 % | byte-identical | 10.8 ns |

With the M4K tiles in the chip database (2026-10-24) all seven examples
were rebuilt with the new nextpnr and validated again: all byte-identical.
picorv32's placement moved (worst setup slack now 0.29 ns, still positive);
the other pre-M4K examples kept theirs or moved and still match.

Tri-state pins go to Quartus as `cycloneii_io` atoms (its VQM reader takes
no `?:`), the way Quartus' own VQM writer has them, with inverted enables as
`~net` wires it folds into `OE_INVERT`. Two more cases, observed
byte-identical without a committed example: an `inout` only read, and one
driven unconditionally. Adding the `OE` pips changed the chip database, and
with it nextpnr's placement of lfsr; that new placement is byte-identical
too, and blinky, lfsr and uart_tx export the same netlist, rcf and RBF with
the old and the new cram. uart_tx and picorv32 were not re-run through
Quartus with the new chip database.

Quartus' fitter accepts each placement and routing (all routes preserved,
no legality errors) and TimeQuest reports positive setup and hold slack.
Built inside `nix develop` (the packaged nextpnr and its chip
database), all four RBFs are identical to the validated
ones, also with `~/altera` hidden and `CRAM_DEVINFO` unset (chip database
and RBFs rebuilt from `db/` alone, byte-identical to those built from
Quartus' files).

**On hardware (2026-10-18):** open-built blinky, and a variant with the LED
order flipped that Quartus never saw, run on a DE2 loaded with `just flash`
(`cram svf` → openFPGALoader, no Altera software); CONF_DONE checked on
every load. See [JTAG configuration](#jtag-configuration-phase-7).

#### M4K in the open flow

A memory with synchronous reads goes yosys → `CY2_M4K` → an M4K bel →
`m4k.rs`'s typed `M4k` → bits, and to Quartus as a `cycloneii_ram_block`
atom. The yosys flow's subset: single port ×1-×18 (read-during-write
returns the new data, the block's flow-through mode), simple dual port
×1-×36 on one clock with the same width on both ports, ROMs (initial
contents on any of them; a ×36 ROM is Quartus' "rom" atom, which it
builds on port B), logical ×8, ×16 and ×32 as physical ×9, ×18 and ×36.

chipsmith's own synthesis (its README, "M4K RAM blocks") builds every mode
above too: byte enables, clock enables, output registers and their clears,
true dual port, mixed widths, two clocks, memories over several blocks and
shallower than one. `cram quartus-export` writes each from chipsmith's
`m4k::Shape` **[obs: 8 chipsmith examples and 16 random designs,
byte-identical, chipsmith README]**:

- operation mode `single_port`, `rom`, `dual_port` or `bidir_dual_port`;
  clocks, clock enables and clears on `clk0/1`, `ena0/1`, `clr0/1`; output
  registers as `port_*_data_out_clock`/`_clear`; a port B clock as
  `port_b_address_clock`, `_read_enable_write_enable_clock`,
  `_data_in_clock`.
- Byte enables: Quartus puts a VQM atom's data bit *k* in column *k*
  whatever `port_a_byte_size` says, so an atom whose bytes sit eight per
  nine columns is written at its physical width (×18, ×36), byte size 9,
  the ninth columns tied off. Quartus' own byte-enabled ×16 puts bit 8 in
  column 9 (fuzz s16b, sublocations), the same bits.
- Two clocks: Quartus 13.0 rejects a `dual_port` atom on two clocks
  (error 15684, Cyclone II errata). Quartus' own two-clock simple dual
  port (fuzz d8k) is a `bidir_dual_port` reading on port A (clock 0) and
  writing on port B (clock 1), port A's write enable and data in tied
  off; chipsmith builds it so, and a true dual port's port that never
  writes keeps `portawe`/`port*datain` tied (errors 14091, 14107).
- Mixed widths: atom bit *k* of the wider port sits in narrow word *k* /
  narrow width, column *k* mod narrow width (no `data_interleave_*`
  parameters needed for a VQM atom).
- Read data of a true dual port's port B leaves on `M4K_BUFFER` 18-35.

- **yosys** (`synth/m4k.txt`, a `memory_libmap` library; `synth/m4k_map.v`).
  libmap's address bus is the block's 12 physical pins, MSB-aligned, and its
  `INIT` is the 4,608 bits in the block's linear order (word *a* of a ×9 port
  at `INIT[a*9 +: 9]`, ×1/×2/×4 skipping every ninth position), so the
  techmap is wiring. A block costs 64, a bit of soft logic 0.025: memories
  of 2,304 bits or less stay logic. That keeps picorv32's 32 × 32 register
  file in LUTs as before (the same cells and connections; some internal net
names differ), and it rules out the
  memories that would leave a block's top address pin unused, which the
  flow can't place yet (below).
- **Mixed-port read-during-write** is declared old data: Quartus' OLD_DATA
  and DONT_CARE give the same bits (fuzz d8o vs d8), so the block does one
  thing and OLD_DATA names it **[inf]**.
- **Chip database**: one tile type `M4K` at logic tile (2x+1, y) of each
  block. Wires `LI0-81` (the block's `LOCAL_INTERCONNECT`), `BCB0-5` (row
  clocks), `MB0-35` (`M4K_BUFFER`, element 278), all aliases of graph
  nodes; `M4K.CTL0-7` (the control inputs, rcf `M4K_LEFT_CONTROL_INPUT`). One
  bel `M4K` (type `M4K`, z 0): inputs `DI0-35`, `WA0-6`, `WA11`, `RA0-6`,
  `RA9-11`, `BE0-3` (named after their asmdb mux, `m4k::Input::pin_name`;
  pips from every line the mux's source map reaches), `CLK0`, `CLK1`,
  `ENA0`, `ENA1`, `CLR0`, `CLR1`, `WE`, `RE` (pips from the control inputs
  and row clocks their fast mux takes), outputs `DO0-35` on `MB0-35`.
- **Packer** (`pack_m4k`): `CY2_M4K`'s logical ports become those pins.
  Data bit *k* is column *k*: `DI<k>`, `DO<k>`. Address pin *p* is
  `m4k::Input::addr` (port A: `DI12-15`, `WA11`, `WA0-6`; port B: `DI26`,
  `DI35`, `RA9-11`, `RA0-6`); pins below the port's width, constant data
  inputs and unread outputs are dropped; a write enable never driven makes
  a ROM. Clocks get a clock control like LE clocks. Buffers in front of an
  M4K input fold away (quartus_map would delete them).
- **Quartus export**: one atom per block, named `m4k_x<X>_y<Y>`, its width the
  bits in use (×16 for a ×18 port with columns 16-17 unused), `mem_init0/1/2`
  in 2,048-bit pieces as Quartus writes them, `M4K_X<X>_Y<Y>` location.
  Observed with hand-written VQMs (q1, q2): quartus_map keeps a VQM RAM atom
  whole, named after its first output net, and the rcf names its sinks on
  that name; bit *k* sits in column *k* (output on `M4K_BUFFER` *k*,
  contents in that order); the rcf's `ram_sublocations` then lists every
  output at (0, 0). A dual port must drive `portbrewe` (error 14091), single
  port and ROM must not (14090). `m4k.rs`'s Quartus frontend reads such
  atoms too (no sublocation: column = bit), so the oracle's own build of an
  exported design roundtrips.
- **Shallower memories** **[obs, one design, q2]**: a 32 × 18 VQM atom (5
  address bits) gets its address on `WA0-4` (pins 5-9), and its words land
  at physical word 192 + 2a: unconnected `WA11` reads 0, `WA5`, `WA6` read 1.
  The rest of the pins' idle levels are unknown, so the packer refuses a
  constant on a used address pin rather than guess where the contents go.

Gaps (this repo's nextpnr flow; chipsmith's own flow has clocks from
logic and PLLs, and the chip database their pips and `PLL` bels; it
also has every M4K mode above but the packed ×36 single port): no timing
model in nextpnr; M4K only in the yosys flow's subset; no multipliers or PLLs; I/O is
3.3-V LVTTL inputs, outputs, tri-state outputs and bidirs only (no I/O
registers, no other standards, drive strengths, pull-ups or bus hold; the
cram model has all of these, the yosys/nextpnr side doesn't); an open
drain written as `d ? 1'bz : 0` becomes a tri-state with constant data, not
Quartus' open-drain mode (both valid, different bits); clocks must come straight from a pin
(no logic-driven or inverted clocks); no inverted async clear; registers
power up 0 (an init of 1 costs inverters).

**Pin settings and I/O registers in chipsmith's flow.** The chip database
gives every user IOE bel three more pins: `INCLK` and `OUTCLK` (the input
register's clock and the one the output and OE registers share), each fed
from the six row-clock buffers (`BLK_CLK_BUF`) of the IOE's tile, a column
IOE's from the tile to its right (every `INCLK`/`OUTCLK` route of the reg
and oereg fuzz designs, 2,061 of them, ends there), and `REGOUT`, which
drives the same two `IO_DATAIN` paths as `COMBOUT`. `cram quartus-export`
passes a routed design's pin settings (`IO_STANDARD`,
`CURRENT_STRENGTH_NEW`, `WEAK_PULL_UP_RESISTOR`,
`ENABLE_BUS_HOLD_CIRCUITRY`, as chipsmith writes them on the pin cells),
its unused-pin mode and its I/O registers (`cycloneii_io` register modes,
power-up values, `inclk`/`outclk`/`regout` routed in the rcf) on to
Quartus, and sets every bank's `IOBANK_VCCIO` to 3.3 V as on the DE2:
without it Quartus takes VCCIO 2.5 V for a bank holding only 2.5-V inputs,
and nCEO's drive with it. chipsmith's README has the results.

Nothing from Quartus gets redistributed. The repo ships only the derived
device database (`db/`, our own format) and our own tools.

## Phases

| # | Phase | Status |
|---|-------|--------|
| 0 | Sandboxed oracle: generated design → chipsmith → `.sof`/`.rbf`, no network | done: `tools/oracle.sh`, verified offline |
| 1 | Container format: SOF/RBF framing, header, CRC, frame layout | RBF layout and every check word known: frames 24+ CRC-16, raw frames 0-23 = 6 CRC'd frames sent 4× (the CFF bits), `cram assemble` writes all of them; header option words not decoded; SOF not looked at |
| 2 | Tile grid: LAB, M4K, multiplier, IOE, PLL coordinates | LAB grid and its CRAM frame/row placement observed; IOE coordinates and their CRAM/CFF bits from pad/pkg + asmdb; the 105 M4Ks (X13/X26/X52, one per LAB row, 70 rows × 166 frames) and 35 multipliers (X39) placed from the asmdb block lists |
| 3 | Logic: LUT truth tables, FF modes, carry chains | asmdb decoded. LUT, LE inputs A-D, carry, registers, packing, LAB clocks/CE/aclr/sclr/sload placed by the asmdb, setting values fit on 9 fuzz designs. All 8 held-out counters and all 9 LE fuzz designs byte-identical with the I/O model |
| 4 | Routing: local, R4/C4/R24/C16 muxes, global clocks | flat CRAM address → frame/bit solved; every routing bit of 17 fuzz designs predicted from ddb, 0 misses; held-out designs reassemble byte-identically. Global clocks (pin → clkctrl → GCLK → RCLK → LAB) modelled: graph plus asmdb clkctrl select, checked on 2 clock pins and 4 global nets. Clock controls fed from the core (a register, the JTAG block's TCK) and their enables: 8 of 8 clock fuzz designs byte-identical |
| 5 | Hard blocks: IOE standards, M4K, multipliers, PLLs | IOE: done for single-ended I/O. Pin → IOE from pad/pkg, every IOE bit from the asmdb, values learned (roles, drive, pull-up, bus hold, I/O registers incl. OE register and power-up high, output enable incl. inversion, tri-state, bidir and open drain, 3.3/2.5/1.8/1.5 V standards, unused-pin modes); held-out random pin sets byte-identical (24/24 held-out output-enable designs). Not covered: LVDS/differential, VREF standards, DDIO, register clear/enable, non-default delay chains. M4K: done for Quartus designs (`cram roundtrip`): single port, simple and true dual port, ROM, ×1-×36 and mixed widths, packed ×32/×36 single ports, output registers, clock enables, clears, byte enables, initial contents, RAMs over several blocks; content layout, input pins and settings from the asmdb, values fit on 28 designs; 52 of 53 held-out designs byte-identical (the miss is an LE clock choice). In the open flow: single port ×1-×18, simple dual port ×1-×36 on one clock, ROMs, initial contents (`examples/ram`, `examples/ram_widths` byte-identical). PLLs: counters, phase, duty cycle, n 1-4, m 4-32, the VCO post-scale, loop and lock settings, both feedback modes, lock output, areset and pfdena from the core, inputs from the clock pins of PLL_1-3; 58 of 59 PLL and clock fuzz designs byte-identical (not the external clock output). In chipsmith's flow (`CY2_PLL`) byte-identical with Quartus. Multipliers not started |
| 6 | Flow: yosys techmap, nextpnr arch, Rust bitstream packer | done for LUT/carry/register logic, global clocks from pins and LVTTL pins including tri-state and bidirectional ones: `just open-build`, no Quartus involved (packing, base image and chip database from the committed `db/`, no Quartus files at use time). M4K RAM (single port, simple dual port, ROM, ×1-×36). The same P&R assembled by Quartus is byte-identical for blinky, lfsr, uart_tx, picorv32, bidir, ram and ram_widths. No timing model, no multiplier/PLL, no registered I/O, M4K without byte/clock enables, output registers, true dual port |
| 7 | Loader: open JTAG programming (openFPGALoader already speaks USB-Blaster) | done: `cram svf` writes the JTAG stream (RBF minus its passive-serial preamble) with a CONF_DONE check; openFPGALoader plays it. Open-built designs run on a DE2. EPCS: the JTAG and serial-flash blocks are modelled (Quartus designs byte-identical, chipsmith's flash loader validates byte-identical) and chipsmith programs the EPCS16 through them |

## Prior art

- [Project Mistral](https://prjmistral.readthedocs.io/): Cyclone V, upstream in nextpnr
- [Cyclone_CRAM_Mapper](https://github.com/14sea/Cyclone_CRAM_Mapper): Cyclone IV EP4CE6, partial routing
- No public Cyclone II effort found as of 2026-10-17

## License

MIT, see [LICENSE](LICENSE). That covers the code and the device database
in `db/`, which is our own format generated by `cram db`; no Quartus files
are in this repository. `examples/picorv32/picorv32.v` is vendored from
[YosysHQ/picorv32](https://github.com/YosysHQ/picorv32) under its ISC
licence.
