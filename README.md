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
  replicated frames, so they hold something outside the routing/asm bit
  space. The earlier "8-byte runs, 96-byte period" was this replication.
  Why the device wants these frames 4 times is open (a slower shift
  clock for that region is a guess).
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
- Bottom-row output pins (still learned, I/O side): 2 bits per pin selecting the local line into
  `IO_DATAOUT`, rows 2476-2477. Odd z mirrors even z about
  `lut_frame(X) − 2`. Learned table (`io_mux_table.py`); lines 2, 15, 17,
  19 and above 20 unseen.
- Everything else seen is I/O configuration for the pin set (6 inputs,
  6 outputs): 260 bits in frames 0-23 and 21 bits in the bottom I/O row,
  identical in every fuzz design. Not decoded.

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

What that does and doesn't show: routing, LUT placement/contents and the
CRAM address map are modelled from first principles plus the device DB.
The output-pin mux is a learned lookup table. The pin I/O config and
frames 0-23 are copied, so a design with a different pin set needs its
own training designs. (Re-run after the asmdb LE model replaced the
learned LE tables: same result, 16 of 17 byte-identical, g2006 as above.)

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
and enable + `!KEY[0]` sync clear. `cram assemble` copies the pin I/O
config leave-one-out from designs with the same pin set:

| design | result |
|---|---|
| c24p, c26k, c28kp, c26e, c22e, cnt26 | byte-identical |
| c20, c32p | 2 data bits: an LED output on a local line the learned I/O mux table hasn't seen |

The 9 fuzz designs reassembled leave-one-out: see the phase table.
c26e found the one rule the fuzz designs had missed: an inverted sclr
clears `LAB_INVERT` 5. The model now tracks port inversion.

Not covered: LAB clocks from local routing rather than a global, two
different aclr signals in one LAB, inverted aclr, a second clock domain
sharing a buffer, `LE_USE_REG_CASCADE_ARM` (never seen changing), M4K,
multipliers and PLLs. These are flagged as `unmodelled` rather than
guessed.

Nothing from Quartus gets redistributed. The repo ships only the derived bit
database and our own tools.

## Phases

| # | Phase | Status |
|---|-------|--------|
| 0 | Sandboxed oracle: generated design → chipsmith → `.sof`/`.rbf`, no network | done: `tools/oracle.sh`, verified offline |
| 1 | Container format: SOF/RBF framing, header, CRC, frame layout | RBF layout and frame CRC known for frames 24+, `cram assemble` writes valid check words; frames 0-23 (content and check scheme) and the header option words not decoded; SOF not looked at |
| 2 | Tile grid: LAB, M4K, multiplier, IOE, PLL coordinates | LAB grid and its CRAM frame/row placement observed; M4K/multiplier/clock columns known only as frame widths |
| 3 | Logic: LUT truth tables, FF modes, carry chains | asmdb decoded. LUT, LE inputs A-D, carry, registers, packing, LAB clocks/CE/aclr/sclr/sload placed by the asmdb, setting values fit on 9 fuzz designs. A real counter assembles byte-identical; 6 of 8 held-out counters are byte-identical, the other 2 miss only an I/O-table entry |
| 4 | Routing: local, R4/C4/R24/C16 muxes, global clocks | flat CRAM address → frame/bit solved; every routing bit of 17 fuzz designs predicted from ddb, 0 misses; held-out designs reassemble byte-identically. Global clocks (pin → clkctrl → GCLK → RCLK → LAB) modelled: graph plus asmdb clkctrl select, checked on 2 clock pins and 4 global nets |
| 5 | Hard blocks: IOE standards, M4K, multipliers, PLLs | not started |
| 6 | Flow: yosys techmap, nextpnr arch, Rust bitstream packer | not started |
| 7 | Loader: open JTAG programming (openFPGALoader already speaks USB-Blaster) | not started |

## Prior art

- [Project Mistral](https://prjmistral.readthedocs.io/): Cyclone V, upstream in nextpnr
- [Cyclone_CRAM_Mapper](https://github.com/14sea/Cyclone_CRAM_Mapper): Cyclone IV EP4CE6, partial routing
- No public Cyclone II effort found as of 2026-10-17
