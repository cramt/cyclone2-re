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
  35 frames per LAB column, 166 per M4K/multiplier column (13, 26, 52), 27
  for the centre column (39), plus 4 unexplained frames before columns 16
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
clocks the LAB. `tools/cram/src/logic.rs` turns that into settings.

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
  enable's line dictates. Otherwise inverted clocks come first, then the
  lowest `BLK_CLK_BUF`. This is fit to 7 two-clock LABs.
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

Not covered: LAB clocks from local routing rather than a global, two
different aclr signals in one LAB, inverted aclr, a second clock domain
sharing a buffer, `LE_USE_REG_CASCADE_ARM` (never seen changing), M4K,
multipliers and PLLs. These are flagged as `unmodelled` rather than
guessed.

### I/O pins (phase 5)

Every pin's I/O element is configured from the device DB: no training
designs and no copied bits. `tools/cram/src/io.rs`.

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
| input | `OE_INVERT`; `PAD_TO_CORE_DELAY`#*k* = 110 for each `IO_DATAIN` index *k* the rcf uses (111 if from the input register) |
| output, bidir | drive code (table below), data mux; output only: `SELECT_IOST` 110 (input buffer off) |
| input buffer | 1.8 / 1.5 V: `SELECT_IOST` 010; 3.3 / 2.5 V: 000 |
| output tied to 0 / 1 | `DATA_INVERT` / nothing |
| pull-up, bus hold | `USE_PULL_UP`, `USE_BUS_HOLD` |
| I/O registers | `*REG_CLK_FAST_MUX` one-hot at row clock buffer + 1 (only buffers 4 and 5 seen, the rest inferred), `USE_CLR_*REG` = 1 (no clear), output register: `REG_FEEDS_PIN_OUT` 10 |
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

What is inferred rather than observed: the CFF lane boundaries inside
unused gaps of the block numbering, and which named setting each learned
encoded value is (pin mode codes, delay codes). Not covered, so `cram`
refuses rather than guesses: LVDS and other differential or VREF
standards, non-default currents outside 3.3-V LVTTL, DDIO, I/O register
clears and clock enables, an OE register without an output register,
programmable delays the fitter only picks under timing pressure, and
PCI clamp (`IO_USE_PCI_DIODE_CLAMP`, never seen set).

### Open flow (phase 6)

Verilog → yosys → nextpnr → `cram pack` → `top.rbf`, with no Quartus
program and no Quartus output involved:

```
nix develop
just chipdb                       # once (~10 min): nextpnr chip database from the committed db/
just open-build examples/blinky   # examples/blinky/build/top.rbf
```

No Quartus files are read at any step: everything comes from the device
database committed under `db/` (below).

**Device database** (`db/EP2C35F672C6/`, `tools/cram/src/db.rs`). What the
use-time commands (`cram pack`, `blank`, `chipdb`, `quartus-export`) need,
derived once from Quartus' devinfo by `just db` (`cram db db`, about 6
minutes) and committed, the way prjxray-db or Apicula ship theirs. Two
xz files:

| file | size | contents |
|---|---|---|
| `device.bin.xz` | 3.2 MB (41.7 MB raw) | magic `C2RE-DB`, format version, then a [postcard](https://postcard.jamesmunns.com/wire-format) encoding of `db::Stored`: the routing graph (376,788 elements, 210,403 templates) and its 7,996,500 routing bits, stored by column as deltas; the asmdb block tables; the 475 bonded user pins of the F672 package with their IOE, bank and dual-purpose role |
| `chipdb.txt.xz` | 9.2 MB (124.5 MB raw) | `cram chipdb` output, the input of `nextpnr/cyclone2/gen_chipdb.py` |

`cram db` writes `device.bin.xz`, reads it back (it must re-encode to the
same bytes), and exports `chipdb.txt.xz` from what it read back, so the
nextpnr generator input can't drift from the packer's database. Output is
deterministic (fixed-size xz blocks), so an unchanged device and unchanged
code reproduce the committed files byte for byte. Rerun `just db` after
changing anything `db.rs` stores or `chipdb.rs` exports, and bump
`db::VERSION` when a stored type changes; a cram that reads another
version refuses the file. cram finds the database through `CRAM_DB` (the
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

**Physical netlist** (`tools/cram/src/phys.rs`). What a placed and routed
design configures in device terms: LE configs (LUT mask over physical
inputs, input sources, carry, register and its LAB clock slot), LAB-wide
settings, clock controls, every used pin's IOE settings, and routed nets as
routing-graph edges. A `Pip` can only be built from two nodes the graph
connects. `bits()` turns it into CRAM. Two frontends build it:
`quartus.rs` (post-fit netlist + rcf + qsf, what `cram assemble` and
`roundtrip` use; still 178/179 byte-identical) and `nextpnr.rs` (routed
himbaechel JSON, `cram pack`).

**Base image without Quartus** (`cram blank`, `tools/cram/src/blank.rs`).
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
→ `nextpnr/cyclone2/gen_chipdb.py` → bbasm). `just chipdb` needs Python
and the committed text only; the 242 MB binary in `build/chipdb/` is not
in git. Each Quartus tile (x, y) is two nextpnr tiles: (2x, y) holds
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
`IO_DATA_INPUT` lines, pad-to-core onto both `IO_DATA_IN` paths or the
clock pin's `CLK_BUFFER`), and 16 clock-control bels sit at their
`CLKBUF_OUT`. Measured: `just chipdb` takes 9.5 minutes and 2.6 GB peak
(himbaechel's python generator and bbasm);
the binary is 242 MB, and nextpnr-himbaechel needs about 330 MB for a
small design.

**nextpnr uarch** (`nextpnr/cyclone2/cyclone2.cc`, packaged by
`nix/nextpnr-cyclone2.nix` against nixpkgs' nextpnr 0.11.1 source; the
devshell's `nextpnr-himbaechel`). Cells come from `synth/cyclone2.ys`:
`LCELL_COMB` (LUT4 + carry), `LCELL_FF` (data, clock, enable, async clear,
sync clear gated by the enable), pin buffers; the packer adds clock
controls. The maps tie every unused cell input (LUT inputs and CIN to 1,
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

Quartus' fitter accepts each placement and routing (all routes preserved,
no legality errors) and TimeQuest reports positive setup and hold slack.
Built inside `nix develop` (the packaged nextpnr, a chip
database from `just chipdb`), all four RBFs are identical to the validated
ones, also with `~/altera` hidden and `CRAM_DEVINFO` unset (chip database
and RBFs rebuilt from `db/` alone, byte-identical to those built from
Quartus' files).

**On hardware (2026-10-18):** open-built blinky, and a variant with the LED
order flipped that Quartus never saw, run on a DE2 loaded with `just flash`
(`cram svf` → openFPGALoader, no Altera software); CONF_DONE checked on
every load. See [JTAG configuration](#jtag-configuration-phase-7).

Gaps: no timing model in nextpnr; no M4K, multipliers or PLLs; I/O is
3.3-V LVTTL inputs and outputs only (no bidirectional pins, output enable,
I/O registers or other standards); clocks must come straight from a pin
(no logic-driven or inverted clocks); no inverted async clear; registers
power up 0 (an init of 1 costs inverters).

Nothing from Quartus gets redistributed. The repo ships only the derived
device database (`db/`, our own format) and our own tools.

## Phases

| # | Phase | Status |
|---|-------|--------|
| 0 | Sandboxed oracle: generated design → chipsmith → `.sof`/`.rbf`, no network | done: `tools/oracle.sh`, verified offline |
| 1 | Container format: SOF/RBF framing, header, CRC, frame layout | RBF layout and every check word known: frames 24+ CRC-16, raw frames 0-23 = 6 CRC'd frames sent 4× (the CFF bits), `cram assemble` writes all of them; header option words not decoded; SOF not looked at |
| 2 | Tile grid: LAB, M4K, multiplier, IOE, PLL coordinates | LAB grid and its CRAM frame/row placement observed; IOE coordinates and their CRAM/CFF bits from pad/pkg + asmdb; M4K/multiplier/clock columns known only as frame widths |
| 3 | Logic: LUT truth tables, FF modes, carry chains | asmdb decoded. LUT, LE inputs A-D, carry, registers, packing, LAB clocks/CE/aclr/sclr/sload placed by the asmdb, setting values fit on 9 fuzz designs. All 8 held-out counters and all 9 LE fuzz designs byte-identical with the I/O model |
| 4 | Routing: local, R4/C4/R24/C16 muxes, global clocks | flat CRAM address → frame/bit solved; every routing bit of 17 fuzz designs predicted from ddb, 0 misses; held-out designs reassemble byte-identically. Global clocks (pin → clkctrl → GCLK → RCLK → LAB) modelled: graph plus asmdb clkctrl select, checked on 2 clock pins and 4 global nets |
| 5 | Hard blocks: IOE standards, M4K, multipliers, PLLs | IOE: done for single-ended I/O. Pin → IOE from pad/pkg, every IOE bit from the asmdb, values learned (roles, drive, pull-up, bus hold, I/O registers, 3.3/2.5/1.8/1.5 V standards, unused-pin modes); held-out random pin sets byte-identical. Not covered: LVDS/differential, VREF standards, DDIO, register clear/enable, non-default delay chains. M4K, multipliers, PLLs not started |
| 6 | Flow: yosys techmap, nextpnr arch, Rust bitstream packer | done for LUT/carry/register logic, global clocks from pins and LVTTL pins: `just open-build`, no Quartus involved (packing, base image and chip database from the committed `db/`, no Quartus files at use time). The same P&R assembled by Quartus is byte-identical for blinky, lfsr, uart_tx and picorv32. No timing model, no M4K/multiplier/PLL, no bidirectional or registered I/O |
| 7 | Loader: open JTAG programming (openFPGALoader already speaks USB-Blaster) | done: `cram svf` writes the JTAG stream (RBF minus its passive-serial preamble) with a CONF_DONE check; openFPGALoader plays it. Open-built designs run on a DE2. Not done: EPCS (non-volatile) programming |

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
