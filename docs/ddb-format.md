# Quartus `.ddb` device database format

How Quartus II 13.0sp1 stores its device databases
(`quartus/common/devinfo/<family>/ddb_*.ddb`), and how `tools/ddb` decodes
the Cyclone II routing graph, wire names and routing configuration bits.

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

Not yet known: what `m_element_enum` values mean. They index
`DEV_ELEMENT_ENUM`. For example, 264 is H4 wires and 43 is an
edge-less per-tile element. **[inf]** The names are in
`DEV_ELEMENT_DESC` (hdr ddb).

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
  2724 - frame = 45-48 falls in that band. **[obs]** So the address is
  probably `row * 2701 + (2724 - frame)`. **[inf]** The row to RBF frame-bit
  mapping is not known yet.

`DEV_ASM_INFO` in libddb_dev.so is different. It holds the `asmdb` and
`asmdb_dirt` files (`ASMDB_DIE`, `ASMDB_*_MUX`, `ASMDB_BLOCK_INFO`),
implemented in libdb_asmdb.so. These look like the block-level (LE, IOE,
RAM) mux-to-bit tables. **[inf]** They are not decoded yet. Their class
table parses.

libddb_bcm2bits.so reads `*.bcm.xml` files. No such files exist for
`cycloneii`, so it plays no part in this family. **[inf]**

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
- `DEV_ELEMENT_ENUM` value names.
- The meaning of exclusion groups, and of the
  `use_encoded_setting`/`is_encoded_bit_high` bits (about 1.5% of bits).
- How the flat CRAM address maps to RBF frame and bit positions.
- `asmdb` (block config muxes), `place`, `timing`, `pad` and `pkg`.
