# Quartus `.ddb` device database format

How Quartus II 13.0sp1 stores its device databases
(`quartus/common/devinfo/<family>/ddb_*.ddb`), and how `tools/ddb` decodes
the Cyclone II routing graph, wire names, routing configuration bits and
the package pin map.

Every claim has a tag:

- **[obs]**: observed. Either read directly in the decompiled reader, or
  checked byte for byte against Quartus's own trace of the same file (see
  [The oracle trace](#the-oracle-trace)).
- **[inf]**: inferred from names, structure or statistics. Not verified.

Addresses are ELF virtual addresses as `nm -D` reports them. Ghidra loads
these `.so` files at +0x100000.

## The oracle trace

`PDB_SEGMENT_BASE::serialize_begin` (libdb_pdb.so @ 0x15380) does
`this[0x188] = cfg_is_on("PDB_ASCII_DUMP")`. With that flag set, every
`PDB_SEGMENT_READER` primitive `printf`s the value it read, the raw delta,
and the C++ member name. The generated serializers set the member name
before each field. **[obs]**

To get a full annotated dump of every database a Quartus tool reads, put
`PDB_ASCII_DUMP=on` in a `quartus.ini` in the working directory and run the
tool. Quartus prints `Info: Using INI file …` when it picks the file up.
`quartus_fit` reads routing (about 1 GB of text for an empty EP2C35 design).
`quartus_asm` reads `asm` and `asmdb` (about 4.9 GB). The trace is
Quartus-derived, so it stays in `specimens/pdbdump/` and is never committed.

This trace is the reference for everything marked [obs] below. Both the
`ddb routing-canon` and `ddb asm-canon` outputs are compared against
canonical forms generated from it. See [Verification](#verification).

## Outer container

`PDB_SEGMENT_READER::read_db_header` (libdb_pdb.so @ 0x101d0). All values
are little-endian. **[obs]**

| off | type | Quartus member | Cyclone II value |
|-----|------|----------------|------------------|
| 0x00 | i32 | `m_version_number` | 0x11 |
| 0x04 | i16 | header id | 0x0bad (0x1bad means "Truncated file") |
| 0x06 | i16 | `m_compression_level` | 7 (xz), 9 (zlib asmdb), 1 |
| 0x08 | i16 | `m_compression_type`, present only if version > 13 | 1 = xz, 0 = zlib |
| 0x0a | i16 | `m_size_of_header` | 0x74 |
| 0x0c | i16 | `m_end_of_header_pos` | 0x74 |
| 0x0e | c_str | creator version string | `Version 13.0.1 Build 232 … SJ Full Version` |
| … | u64 | `m_size_of_body` (uncompressed) | |
| … | u64 | `m_end_of_body_pos` (file offset) | |
| … | u64 | `m_size_of_footer` (uncompressed) | |
| … | u64 | `m_end_of_footer_pos` (= file size) | |

The u64 fields exist only when version == 0x11. The body is compressed at
file range `[end_of_header_pos, end_of_body_pos)` and the footer at
`[end_of_body_pos, end_of_footer_pos)`. Each is a separate stream through
`CLW_COMPRESSION_STREAM_FACTORY::create_stream(type)`. **[obs]**

- xz: a standard `.xz` stream. **[obs]**
- zlib: a zlib header and deflate data, ended by a sync flush with no final
  block and no adler32. Strict decoders report a truncated stream. Feeding
  the exact range and accepting EOF gives exactly `m_size_of_body` bytes.
  **[obs]** This applies to `asmdb`, `asmdb_dirt`, `lei`, `pwr` and `blkrba`.
  They were the "no XZ stream" files.

All 180 `cycloneii/*.ddb` files parse with this layout: `ddb info` on every
file, sizes match. **[obs]**

## Footer: the class table

`PDB_SEGMENT_READER::read_db_footer` (@ 0xfbf0). **[obs]**

```
i16 n_classes
repeat n_classes:
  i16   class_id          # the id used in pointers in the body
  i32   class_size        # sizeof the in-memory C++ object
  i32   hash_value        # schema hash; a mismatch is "Incompatible database version"
  i32   number_of_instances   # 0 in every file seen
  c_str class_name        # e.g. "DYGR_ROUTE_ELEMENT_TEMPLATE"
  i8    processed
  if processed == 0:
    i32 largest_base_public_id    # asserted == 0
    i32 base_pointer_table_size   # capacity of that class's pointer table
```

The hash pins the field layout. Quartus refuses to load a file if the hash
differs from the one compiled into the reader. Any file with the same
`(class_name, hash)` has the same layout. **[obs]** for the refusal and
**[inf]** for the layout claim.

## Body primitives

No type tags exist in the stream. The reader must know the schema. The
`PDB_SEGMENT_READER` vtable gives the primitive set (slot: function): 0x50
`get_c_str`, 0x58 `get_char`, 0x60 `get_short`, 0x68 `get_int`, 0x70-0xa8
`xfr_*_delta`, 0xb8 `xfr_c_str_v`, 0xc0-0xf8 `xfr_char` through
`xfr_double`, 0x100 `xfr_bool`, 0x108 `xfr_field_v`, 0x130 `xfr_new`.
**[obs]**

- **Plain scalars** are raw LE: i8, i16, i32, i64, f32, f64. A bool is one
  byte, nonzero meaning true. **[obs]**
- **`c_str`** is an i32 `n` followed by `n-1` bytes. The length counts a NUL
  that is not stored. `n == 0` is a null pointer. **[obs]** (`get_c_str` @ 0xf1e0)
- **`std::string`** is an i32 `n` followed by exactly `n` bytes. **[obs]**
  This is the icname decode, consumed to the exact end of the body.
- **Bitfields** use `xfr_field_v(idx, bits, base)` (@ 0x17410). Widths of 8
  bits or fewer read an i8, 16 or fewer an i16, otherwise an i32. The base
  is added to the value. **[obs]** 1-bit signed bitfields store true as -1
  (0xff). **[obs]**
- **Deltas** (`xfr_*_delta`, `xfr_field_v`): value = raw + the same field in
  a *reference object*. The reference is located by the byte offset
  `PDB_SEGMENT_BASE+0x190`. **[obs]** Which object that is, as seen in the
  trace:
  - Class objects, whether inside a vector or behind pointers, reference the
    previously serialized object of the same class. Fields of the first
    object reference a zeroed buffer (`serialize_begin` allocates and
    `memset`s it, `+0x180`). For example, `DYGR_ROUTE_ASM_BIT::m_flat_address`
    chains straight through all 376,788 ASM nodes. **[obs]**
  - Primitive vector elements reference the previous element. Element 0
    references element 0 of the same member in the previous object, or 0 if
    that vector was empty. **[obs]** for `DYGR_ROUTE_ASM_NODE::m_start_bit_index`.
    The routing vectors (`m_fanout_list_offsets`, `m_fanout_to_positions`)
    always have element 0 == 0, so they don't distinguish the rules.
  - `std::map` keys: delta to the previous key, the first against 0. **[obs]**
- **Containers** (`std::vector`, `std::map`): an i32 count, then the
  elements. **[obs]**

### Pointers

`read_ptr(PDB_OBJECT_INFO*, bool)` (@ 0x17a80) reads one i32 `v`. **[obs]**

- `v == 0`: null.
- `v < 0`: a new object of footer class id `-v`. It takes the next id in
  that pointer table. The ids are sequential, and the first is 1. **[obs]**
  The object body is **not** inline. Bodies of pointed-to objects are
  serialized later, in id order, by `PDB_OBJECT_INFO::finish_reading`. The
  body's top-level object is the exception: it is `ff ff ff ff` (class 1)
  at offset 0, and its fields follow immediately. **[obs]**
- `v > 0`: a back-reference. id = `(last_id + v) & 0x3fffffff`, where
  `last_id` is the previous id resolved in the table. The writer sets bit
  30 so that negative relative steps still encode as a positive i32.
  `0x40000001` means "next id". `0x7ffffff1` means "15 back". **[obs]**

## Cyclone II databases decoded so far

The magellan family is Cyclone II. magellan4 is the EP2C35. Unless marked
otherwise, the counts are for magellan4.

### `icname`: `DYGR_ROUTE_ICNAME_INFO_BODY`

The body holds two `std::map<unsigned, std::string>`: `ic_name`, then
`ic_sch_name`. Each map is an i32 count, then per entry a delta-encoded u32
key and a `std::string`. The serializer is a template instance at
libddb_dygr.so 0x2073a0. **[obs]**

- 149,468 entries in both maps, with identical key sets. **[obs]**
- Keys are routing global ids. Bit 31 set means a routing element, and the
  low 31 bits index `m_route_element_list`. `is_router_id` is @ 0xdd260 and
  `get_node_index_of_id` is @ 0xdd280. **[obs]** Here the node indices run
  from 99,664 to 348,663. **[obs]**
- `ic_name` values are wire names such as `H4R35C66RT[0]`, `V16_C1UP_R[1]`
  and `GCLK0CS49B`. `ic_sch_name` values are such as `XQ4/Q4_H4R35C66RT[0]`,
  or `-` if there is none. **[obs]**
- The other 227,320 routing elements have no icname entry. These are block
  ports, LE/LAB inputs and similar. **[inf]** Their names come from
  `DYGR_ROUTE_BLOCK_INTERFACE*` (strings such as `IP:LEIM0`, `LL:3`, `LO:0`
  in the routing body), which is not decoded yet.

### `routing`: `DYGR_ROUTE_INFO_BODY`

The body begins with these members, in order. **[obs]**

```
ptr  (new, class 1 = DYGR_ROUTE_INFO_BODY)
i32  n = 376788                       m_route_element_list
n x DYGR_ROUTE_ELEMENT                # deltas vs the previous element
     i16  m_x
     i16  m_y
     u32  m_fanout_edge_base          # a global id
     ptr  m_template                  # -> DYGR_ROUTE_ELEMENT_TEMPLATE
i32  210403                           m_route_element_templates (ptr vector, all back-refs)
...  lookup table, element list info, block types, block interfaces,
     tile offsets, string<->index maps            (NOT DECODED)
...  deferred objects: DYGR_ROUTE_BLOCK_INTERFACE, _INTERFACE_NODE,
     _BLOCK_TYPE, _BLOCK_NODE, LOOKUP_INFO, DYGR_LOOKUP*  (NOT DECODED)
210403 x DYGR_ROUTE_ELEMENT_TEMPLATE  # last in the body, in id order
     i16  m_element_enum              # deltas vs the previous template
     i16  m_num_edges
     i16  m_z
     i16  m_index
     i16  m_first_purely_redundant_edge
     i8   m_length                    # bitfields, 1 byte each
     i8   m_metal_layer
     i8   m_direction
     i32 k, k x i32  m_fanout_list_offsets   # deltas vs the previous element
     i32 k, k x i16  m_fanout_to_positions
```

The 16-byte in-memory `DYGR_ROUTE_ELEMENT` is `{i16 x, i16 y, i32
fanout_edge_base, TEMPLATE*}`. Fanout edge k of element i goes to global id
`template.offsets[k] + element.fanout_edge_base`
(`DYGR_ROUTE_ELEMENT::get_fanout_incl_purely_redundant_edges` @ 0xe7180).
**[obs]** Templates are shared: all elements with the same type, z/index,
length and relative fanout pattern point to one template. 376,788
elements share 210,403 templates. **[obs]**

The middle section isn't decoded. The parser finds the template bodies by
searching for the unique start offset from which exactly 210,403 templates
parse to the exact end of the body. **[obs]** that this works: the output
is identical to the trace. It is still a workaround. Decoding the middle
section properly would replace the search and also give the block-port
names.

Graph facts for magellan4. **[obs]**

- 376,788 routing elements and 3,104,196 fanout edges.
- Every fanout target is a routing id (bit 31 set) inside the element list.
- `first_purely_redundant_edge == num_edges` in every template, so this die
  has no redundant edges.
- Fan-in is not stored. It is the inverse of the fanout lists, and the
  `DYGR_ROUTE_FANIN_MAP` is built at load time. `tools/ddb` computes it.

Not yet known: the names of most `m_element_enum` values. They index
`DEV_ELEMENT_ENUM`. The ones that rcf back-annotation names: 45
`LE_BUFFER`, 276 `LOCAL_INTERCONNECT`, 264 `R4` (H4), 268 `R24`, 269
`C4` (V4), 275 `C16`, 289 `CLK_BUFFER`. **[obs]**: with this table every
consecutive resource pair in 17 fuzz rcf files is exactly one fanout edge.
The names are in `DEV_ELEMENT_DESC_BODY` in the hdr ddb (`m_int_data[0]` =
enum, `m_string_data[0]` = name). 43 is `LOCAL`, the LE output feeding back
into its own LAB, and 276 is `LAB_LINE`. See
[timing-model.md](timing-model.md#element-enum-names). **[obs]**

### `asm`: `DYGR_ROUTE_ASM_INFO_BODY`: routing bits

This file maps routing connections to configuration-RAM bits. **[obs]**
for the structure and **[inf]** for the CRAM interpretation.

```
ptr  (new, class 1)
i32  m_num_asm_nodes = 376788         # one per routing element, same order
i32  376788                           m_route_asm_nodes
n x DYGR_ROUTE_ASM_NODE               # scalars: deltas vs the previous node
     i16  m_num_bit_groups
     i16  m_num_bits
     i32 g, g x i16  m_start_bit_index   # 1-based start of each group in m_asm_bits
     i32 b, b x DYGR_ROUTE_ASM_BIT       # deltas vs the previous ASM_BIT, file-wide
          i32 m_flat_address
          i8  m_use_encoded_setting   # 1-bit signed bitfields: -1 = true
          i8  m_is_encoded_bit_high
          i8  m_is_strangely_encoded
          i8  m_is_cff_bit
```

The in-memory `DYGR_ROUTE_ASM_BIT` is 5 bytes: a 31-bit flat address and
flag bits (constructor @ 0xec440, `get_flat_address` @ 0xec4c0, which ORs
in 0xa0000000 for one flag). **[obs]**

The groups correspond to fanout edges of the **source** element.
`get_destination_bit_group(src, dst)` (@ 0xeca70) returns the index of
`dst` in `src`'s fanout list. **[obs]** Groups at index
`first_purely_redundant_edge` or later are "exclusion groups"
(`get_exclusion_bit_group` @ 0xec980). **[obs]** In magellan4:

- 7,996,500 ASM bits in total.
- 87,301 nodes have exactly one group per fanout edge.
- 233,388 nodes have extra groups beyond their fanout count. These are
  exclusion groups. Even wires with 0 fanouts have them, such as
  `H4R17C58LT[11]`: 2 groups, 7 bits.
- Most fanout edges set 2 bits. This fits a two-level (row x column) mux
  encoding. **[inf]**
- Consecutive bits in a group mostly differ by 2697-2710 or by small
  steps. Addresses run from 6,715 to 6,707,826. **[obs]** Take addresses
  mod 2701, which is 2725 RBF frames minus the 24 differently-checked
  frames 0-23. The residues for the muxes feeding LAB X64Y19 are 27-75.
  For X63Y19 they are 62-109: +35 per X step, which equals the RBF's 35
  frames per LAB column. The LUT of X64Y19 is in frames 2676-2679, and
  2724 - frame = 45-48 falls in that band. **[obs]**
- The full map, `frame = 2724 - addr % 2701` and `row = addr / 2701` laid
  out as 8 bit-in-byte lanes, predicts every routing bit of the 17
  routing fuzz designs with no misses and no polarity errors. **[obs]**
  The formula and the LAB/LE layout in those rows are in the README
  ("CRAM addressing"); `tools/cram/src/addr.rs` implements it.

`DEV_ASM_INFO` in libddb_dev.so is different. It holds the `asmdb` and
`asmdb_dirt` files, implemented in libdb_asmdb.so. `asmdb` is decoded
below.

### `asmdb`: `ASMDB_DIE`: block configuration

The body is zlib-compressed (see the outer container). `tools/ddb/src/asmdb.rs`
reads it, and `ddb asmdb-canon` matches the trace. **[obs]**

- **No class tags on deferred bodies.** The reader knows each pending
  object's class from the `new` pointer that created it. Pointer tables
  are drained LIFO: the most recently created table with pending objects
  is written next, completely and in id order. A table that gets new
  objects after being drained goes back on top. The tables are arch groups
  (`ARCH_GROUP_CALC/STD/DUMB/STLMAP*`), muxes (all `*_MUX` classes),
  defaults (`DEFAULT_MUX_SETTING` + `DEFAULT_BIT_CALC`), block infos and
  index maps. **[obs]**
- Strings are an i32 exact length plus bytes, with no NUL counted
  (unlike `c_str` in routing). Containers held by pointer (block lists,
  index maps, export maps) are preceded by a one-byte presence flag.
  **[obs]**
- Deltas: `m_mux_bits` element 0 is a delta against element 0 of the
  previous mux of *any* class (they share the `ASMDB_MUX` base);
  `ARCH_GROUP_DUMB::m_bits` works the same way. Block list vectors and
  `m_simple_block_lists` values restart from 0 in every object. **[obs]**
- `ASMDB_DIE`: bit counts, three strings, `m_block_info_map` (block
  type → `BLOCK_INFO`), 529 arch groups, 46 defaults, 928 default bits,
  4 labelled index muxes (`le_abs2loc`, `le2lab`, `row_abs2loc`,
  `row2mac`), and five empty vectors. **[obs]**
- `ARCH_GROUP_BASE`: `m_group_index` (delta), `m_type`
  (`ASMDB_ARCH_GROUP_TYPE`), `m_group_block_type`. CALC/STD continue
  with `m_mux_path_list` (per path, a list of mux pointers, one per mux
  level) and `m_mux_data_list` (`arch_value`, `aux_mux`,
  `next_mux_selection`). CALC adds `m_block_info` and two index-map
  mux pointers. DUMB is `arch_value` plus a bit list. **[obs]**
- The type names are the libdb_asmdb.so translator table: 4,585 string
  pointers (`.data.rel.ro` 0x30c160, 64-bit lib), indexed by enum value.
  Arch groups of one block type all get sensible names under it (LAB =
  18: `LE_LUT_INPUT`, `LAB_CLOCK_FAST_MUX`, ...; IOE = 5/6; M4K = 39;
  MAC = 41; PLL = 43; clkctrl = 75). **[obs]** for the table, **[inf]**
  that it is exactly what `asmdb_get_debug_string` returns.
- `BLOCK_INFO` (CALC groups): one `row << 16 | column` base per block
  instance. A mux bit is a flat-address offset from that base. LAB
  blocks are 70 × 35 (`m_simple_block_lists`) and there are 2,076 of
  them. **[obs]** STD/DUMB groups (IOE, clkctrl) hold absolute flat
  addresses. **[obs]** on the clkctrl groups.
- Mux semantics, from comparing against fuzz designs: a path is one
  instance (one per LE for LE groups). For a source mux, `aux[source]`
  is the first-level bit index and `next[source]` the second-level one
  (−2 = unreachable). An encoded single-level mux with setting s sets
  bit s−1. A `DECISION_MUX` has a −3 sentinel plus one real bit. **[obs]**
  on the LAB and clkctrl groups, not checked elsewhere.

#### IOE groups and CFF bits

Block types 5 (row IOE, left and right edges), 6 (column IOE, top and
bottom), 60 and 61 (dedicated clock input, p and n pad) hold the I/O
element features: 43, 42, 2 and 1 groups. `tools/cram/src/io.rs` uses
them. **[obs]** for everything below unless tagged.

- They are STD and DUMB groups, so their bits are absolute. A muxed
  group has one path per IOE (239 row, 220 column, 8 + 8 clock). A DUMB
  group lists one bit per IOE in the same order: its pull-up and bus-hold
  blocks agree with the muxed groups' drive-strength blocks on all 459
  IOEs.
- Path *i* of block 5 (6) is the *i*-th `Row I/O` (`Column I/O`) pad of
  `ddb_<die>_pad` in pad-id order. Dedicated clock pads pair up in pad-id
  order, pair *i* being path *i*; the `p` pad of the pair is in block
  60, the `n` pad in 61 (pad `alt_name` ending in `A`). Checked against
  every output-data mux of the I/O fuzz designs and the 16 clock pins.
- A bit value is a flat CRAM address (`>= 0`), `-3` for a slot this IOE
  doesn't have (column IOEs lack the LVDS input or output select, row
  IOEs the duty-cycle bits), or `0xa0000000 | k` for **CFF bit** *k*.
  The asmdb die header counts 12,456 CFF bits; they are what the 6
  replicated frames at the front of the RBF hold.
- CFF bits come in blocks of 24 (*j* = *k* / 24): one block per IOE,
  one per dedicated clock input, eight for the PLLs (*j* 0-7). Block *j*
  lives in one bit-in-byte lane of the replicated frames at byte slot
  *u* = *k* − off, frame *u* / 311, byte *u* % 311 + 1. Fit, **[obs]**
  on every IOE bit of the I/O fuzz designs:

  | lane | first block *j* | off |
  |---|---|---|
  | 7 | 0 | −216 |
  | 6 | 64 | 1176 |
  | 5 | 126 | 2808 |
  | 4 | 190 | 4440 |
  | 3 | 266 | 6024 |
  | 2 | 324 | 7368 |
  | 1 | 388 | 9120 |
  | 0 | 452 | 10704 |

  The lane boundaries fall between the dedicated clock blocks; where
  exactly in the unused gaps (e.g. *j* 122-125) is **[inf]**.
- `IO_DATA_INPUT` (output data) and `IO_PIA_TO_OE` (output enable) are
  two-level source muxes like the LAB's: `aux[line]` is the first-level
  bit, `next_mux_selection[line]` the second, the source index being the
  LAB-local line (`LOCAL_INTERCONNECT` index in the rcf) that feeds the
  pin. 21 sources on column IOEs, 42 on row IOEs. Column IOEs 0 and 1
  take both from the local lines of the tile at X+1 (rcf `IO_DATAOUT` and
  `IO_OE_PIN`).
- `IO_PIN_MODE` (2 bits, encoded map 1,1,1,3,3,0,2): bit 0 is 1 for a
  combinational output enable and 0 when the OE register drives it; bit 1
  is open drain (`open_drain_output`). There is no `IO_USE_OPEN_DRAIN`
  group in this family. `IO_OE_INVERT` inverts the enable after its mux;
  an input or a never-enabled output sets it. `IO_*REG_POWERS_UP_HIGH` is
  a register's power-up value. Observed on the output-enable fuzz designs
  (`tools/gen_oe.py`).
- No slew-rate feature exists for these blocks (`IO_USE_SLOW_SLEW` and
  `MIO_SET_*SLEW*` are absent), so Cyclone II slew is not configurable.

#### M4K groups

Block type 39 has 132 groups, each with one path; their
bits are offsets from one block's base (`BLOCK_INFO`: 105 bases, one per
LAB row in three columns, block size 70 rows × 166 columns).
`tools/cram/src/m4k.rs` uses them; the README's M4K section has the
values. **[obs]** for everything below unless tagged.

- `RAM_CONTENT_YGR_MEAB` is a single `RAM` mux (`ASMDB_RAM_MUX`, no
  polarity mux) of 4,608 bits: the memory, bit `col × 128 + row` of the
  128 × 36 array.
- The 88 `RAM_MS_*_INPUT` and `RAM_PIA_TO_INPUT_YGR` groups are two-level
  source muxes (a `MAPPED_SIMPLE` first level of 10 entries, a simple
  second level of 3) over 87 sources; the source is the rcf
  `LOCAL_INTERCONNECT` index at the block's tile. Level-0 entries of −5
  are padding no source maps to. `DATA_INPUT` 18+k and `DATA_INPUT_B` k,
  `BYTE_ENABLE_INPUT` 2+k and `_B` k list the same bits.
- The control fast muxes (`RAM_CLOCK_`, `_CE_`, `_CLEAR_`, `_WE_`,
  `_RE_FAST_MUX`) are `MAPPED_ENCODED`: the map, indexed by source (0-7
  control inputs, 8-13 row clocks; on some muxes a last entry holds the
  unused value), gives the bit pattern to
  write, bit *i* of the value being mux bit *i*. −2 marks a source the mux
  can't take. The same reading fits `RAM_DATA_WRITE_MODE` (map index →
  bit number), `RAM_REG_DATA_OUT` (map [1, 0]) and the unused-block values
  `cram blank` writes.
- `RAM_USAGE_MODE` (26 bits, map of 5) never changed in any design;
  what it encodes is open.

libddb_bcm2bits.so reads `*.bcm.xml` files. No such files exist for
`cycloneii`, so it plays no part in this family. **[inf]**

### `pad` and `pkg`: package pins to IOE locations

`ddb_<die>_pad.ddb` holds the die's pads and `ddb_<die><package>_pkg.ddb`
(e.g. `magellan4fbga672`) the package balls. `tools/ddb/src/pins.rs`
decodes both to the exact end of the body. **[obs]**

Both use one generic "info body" schema. Each object has fixed arrays
`m_bool_data`, `m_int_data[N]`, `m_string_data[N]`, `m_vec_int_data[N]`,
`m_2d_int_data[N]` and so on. The slot index is a per-class enum whose
names are not stored. A `*_enum_is_valid` bit vector per array says which
slots are populated. **[obs]** for the layout, **[inf]** for the enum
reading. Encodings seen here, in addition to the ones above: **[obs]**

- `std::vector<bool>`: an i32 bit count, then `ceil(n/32)` i32 words.
  Each word is a delta with the same rule as other vector elements.
- Fixed arrays such as `m_int_data[53]` chain like vectors: element 0
  references element 0 of the previous object of the class, and element
  *i* references element *i-1*.
- Strings are an i32 `n`, then exactly `n` bytes. The trace shows a NUL,
  but none is stored. `n == 0` is an empty slot.
- `vector<vector<int>>`: a row count, then per row a count and chained
  ints. `vector<double>`: a count and raw f64s.
- `map<int,int>` and `map<string,int>`: keys and values chain from 0.
- `DEV_PIN::m_pad_id_list[0]` references 0, not the previous pin's list.
  So the vector rule above is not universal.
- Enum vectors (`m_io_standard_enums`, `m_current_set`) are one i32 per
  element, which the trace prints twice.

Deferred bodies come in this order: `DEV_IO_STANDARD_DESC` (each just
`ptr m_body`), `DEV_IO_STANDARD_DESC_BODY`, the default `DEV_PAD_BODY`,
`DEV_PAD` (each `ptr m_body`), then one `DEV_PAD_BODY` per pad. **[obs]**

```
DEV_PAD_INFO_BODY                        # magellan4: 487 pads
  ptr vector m_pad_list -> DEV_PAD, ptr m_default_pad_body,
  info arrays, m_ids_by_global_id, m_ids_by_string,
  m_io_standards (39), m_io_standards_by_string,
  m_default_io_standards_by_enum[105], m_io_standard_enums, ptr m_io_rules
DEV_PAD_BODY
  i32 m_id (delta), ptr m_default_pad_body, bool[155], int[53],
  string[20], 2d_int[10], string_hash[4], 5 valid vectors,
  ptr vector m_io_standards, ptr m_io_standards_by_enum[105]
DEV_PKG_INFO_BODY                        # fbga672: 672 pins
  ptr vector m_pin_list -> DEV_PIN, info arrays, 11 valid vectors,
  m_ids_by_string
DEV_PIN (bodies follow in id order)
  i32 m_id (delta), bool[46], int[24], string[12], vec_int[7],
  string_hash[4], 5 valid vectors, vector<int> m_pad_id_list
```

Slots that have a meaning:

| member | meaning | tag |
|--------|---------|-----|
| `DEV_PIN::m_string_data[0]` | ball name (`N25`) | [obs] |
| `DEV_PIN::m_string_data[1]` | power function of balls with no pad (`VCCD_PLL3`) | [inf] |
| `DEV_PIN::m_pad_id_list` | `DEV_PAD_BODY::m_id` of the bonded pad, 0 or 1 entries | [obs] |
| `DEV_PAD_BODY::m_id` | pad id, equal to the fit report's "Pad Number" | [obs] |
| `DEV_PAD_BODY::m_string_data[1]` | `X<x>Y<y>SUB_LOC<z>`: IOE tile and index | [obs] |
| `DEV_PAD_BODY::m_string_data[3]` | `Row I/O`, `Column I/O`, `Dedicated Clock`, `Dedicated Programming`, `JTAG` | [obs] |
| `DEV_PAD_BODY::m_string_data[4..6]` | function (`LVDS151n`), pad name (`P0B0`), second name (`LVDSB0A`) | [inf] |
| `DEV_PAD_BODY::m_int_data[0]` | global id, the key of `m_ids_by_global_id` | [obs] |
| `DEV_PAD_BODY::m_int_data[2]` | I/O bank - 1 | [obs] |
| `DEV_PAD_BODY::m_int_data[6]` | differential partner pad id | [inf] |
| `DEV_PAD_BODY::m_int_data[11]` | VREF pad id of the group (0 on VREF pads) | [inf] |
| `DEV_PAD_BODY::m_int_data[12]` | VREF group, 0..15 | [inf] |
| `DEV_PAD_BODY::m_io_standards` | supported I/O standards, named by `DEV_IO_STANDARD_DESC_BODY::m_string_data[0]` | [inf] |

On fbga672, 487 balls have one pad each and 185 have none (power,
ground, PLL supplies). `ddb pins <devinfo> magellan4 fbga672` prints the
join as CSV. `ddb_cycloneii_pad.ddb` uses the same schema with no pads.
It holds 5 family-wide I/O standards and is not needed for the pin map.
All 26 Cyclone II `pad` and `pkg` files decode. **[obs]**

## Verification

`tools/ddb` has two commands that print a canonical text form:
`routing-canon` (one line per element and per template) and `asm-canon`
(one line per ASM node). The scripts in `tools/ddb/verify/` build the
same form from the `PDB_ASCII_DUMP` trace. For magellan4:

- Routing elements (376,788 lines) and templates (210,403 lines) are
  byte-identical. **[obs]**
- ASM nodes (376,788 lines) are identical. The trace prints the -1 flag
  values as 32-bit `FFFFFFFF`, so it is compared after normalizing that.
  **[obs]**
- `pad` and `pkg` (magellan4, fbga672, and `cycloneii_pad`): `ddb
  pins-trace` prints every decoded value in trace order. The output is
  identical to the trace's value column (128,949, 47,392 and 1,105
  values). **[obs]** Checked against all 91 fit reports in
  `specimens/`: the pad number, bank and I/O type of every ball (91 x
  672) match. The X/Y/cell of every placed I/O (2,035 rows, 465 of the
  487 user balls) also match. **[obs]**
- `icname` was not in either trace, because neither tool loads it. Its
  decode consumes the body to the exact last byte, and both maps have the
  same 149,468 keys. **[obs]** that it parses cleanly. It is not checked
  against Quartus.

`ddb export` also runs cleanly on the other five Cyclone II dies. Every
consistency check passes: exact end of body, matching icname key sets,
ASM node count equal to element count, and all edges inside the graph.
**[obs]**

| die | routing elements | fanout edges | icname | ASM bits |
|-----|------|------|------|------|
| magellan1 | 62,667 | 461,290 | 26,400 | 1,192,606 |
| magellan2 | 103,817 | 800,553 | 42,812 | 2,062,306 |
| magellan3 | 217,359 | 1,750,690 | 87,632 | 4,534,327 |
| magellan4 (EP2C35) | 376,788 | 3,104,196 | 149,468 | 7,996,500 |
| magellan5 | 562,838 | 4,700,765 | 222,520 | 12,075,858 |
| magellan6 | 776,454 | 6,552,357 | 304,088 | 16,789,430 |

## Open questions

- The middle of the routing body: block interfaces and lookup tables. This
  section is needed for non-wire node names (`IP:LEIM*`, `LL:*`, `LO:*`)
  and for (x, y, type, index) to element lookup.
- `DEV_ELEMENT_ENUM` value names beyond the routing ones in
  [timing-model.md](timing-model.md#element-enum-names).
- The meaning of exclusion groups, and of the
  `use_encoded_setting`/`is_encoded_bit_high` bits (about 1.5% of bits).
- `asmdb`: MAC and PLL group semantics (decoded, unused); M4K
  `RAM_USAGE_MODE` and the address-stall inputs; IOE
  features the I/O fuzz designs never exercised (DDIO, LVDS, register
  clears/enables, non-default delay chains); `asmdb_dirt`, `place` and
  `timing`.
- Most `pad`/`pkg` info-body slots. Only the ones in the table above are named.
