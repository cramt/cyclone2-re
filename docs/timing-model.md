# Cyclone II timing model in Quartus II 13.0sp1

Where Quartus keeps the delays for the EP2C35F672C6 (die `magellan4`, speed
grade 6), how the files are laid out, and how far the decoded numbers have
been checked against `quartus_sta`. The container format and the PDB
encoding rules are in [ddb-format.md](ddb-format.md).

Every claim has a tag:

- **[obs]**: observed. Checked against Quartus's own `PDB_ASCII_DUMP` trace
  of the file, against `quartus_sta` reports, or read in disassembly.
- **[inf]**: inferred from names, structure or statistics. Not verified.
- **[unk]**: unknown.

## Summary

| What | File | Decoded | Checked against quartus_sta |
|---|---|---|---|
| Cell delays: LUT, carry, register tco/tsu/th, IOE, clock buffers, M4K, multiplier, some clock-network hops | `ddb_magellan4-<corner>.dmf.ddb` (DTM) | Fully; `ddb dmf` | 53/53 distinct slow-model LE values (22,513 report rows) and 47/47 fast-model values (22,270 rows) |
| Physical routing parameters: wire length, R and C per micron, driver model, load positions | `ddb_magellan4_timing.ddb` | Per-element part; `ddb route-timing` | Indirectly (all 848 STA routing elements join to a decoded element) |
| Driver/switch models that turn R, C and loads into delays | `ddb_cycloneii-<corner>_nlspcdata.ddb`, `_nlspccache.ddb` | Schema only, from the trace | No |
| The fitter's per-element-type delay estimates | `ddb_magellan4-<corner>_fdi.ddb` | Schema and per-type averages, from the trace | Compared to STA per-type medians (table below) |
| Per-design computed routing delays | project `db/` (`TIM_RE_DELAY_TREE_*`) | No | No |

**Routing delays are not stored anywhere in devinfo.** **[obs]** for the
inputs, **[inf]** for the mechanism. During "delay annotation" the fitter
computes each routing element's delay from that element's length, R/um, C/um,
driver model and the positions of its loads (`timing`), using the
nonlinear driver and switch models (`nlspc`). It writes the results into the
project database, and `quartus_sta` reads them back from there. Delays of the
same element differ by destination and transition: 214 of 1,029 elements in
the test design have more than one delay in the STA report (for example
`LOCAL_INTERCONNECT_X41_Y2_N0_I34` has 0.367 and 0.379 ns). **[obs]**

Cell delays, in contrast, are plain integers in picoseconds in the DMF, and
`quartus_sta` reports them unchanged. **[obs]**

## Which files are read

Tracing `quartus_fit` with `PDB_ASCII_DUMP=on` (method in
[ddb-format.md](ddb-format.md#the-oracle-trace)) on the blinky design gives
these timing-related reads, matched to devinfo files by uncompressed body
size: **[obs]**

| Body size | File | Classes |
|---|---|---|
| 259,845 | `ddb_magellan4-6.dmf.ddb` (read twice) | `DTM_*` |
| 3,752,824 | `ddb_magellan4_timing.ddb` (read twice) | `DYGR_ROUTE_TIMING_INFO_BODY`, `DYGR_TIMING_*`, `DYGR_PACKED_ARRAY`, `DYGR_RC_TEMPERATURE_INFO` |
| 1,164,226 | `ddb_magellan4-6_fdi.ddb` (read twice) | `DYGR_FITTER_DELAY_*`, `DYGR_DELAY`, `DYGR_MULTI_DELAY` |
| 483,708 | `ddb_cycloneii-<corner>_nlspcdata.ddb` (12 reads) | `NLSPC_*`, `t_nlspc_driver_model`, `t_nlspc_cache_params_for_model` |
| 522,796 / 358,364 | `ddb_cycloneii-m_nlspccache.ddb`, `ddb_cycloneii-6_nlspccache.ddb` | `NLSPC_CACHE_DATA`, `t_nlspc_interp_cache`, `t_nlspc_reducedwave` |

All nine `nlspcdata` files have the same body size, so which corners the 12
reads are is **[unk]**. The `-m` cache shows up next to the `-6` one, so the
fitter probably loads the fast corner models too. **[inf]**

`quartus_sta` on an already-fitted project reads only the DMF, plus a
project file with the `TIM_RE_DELAY_TREE_NODE`, `TIM_KEY_TO_DELAY_MAP*` and
`TIM_MANAGER_IMPL` classes. It reads neither `timing`, `nlspc` nor `fdi`,
even after deleting `db/top.sta_cmp.6_slow.tdb`. **[obs]** The project file
is probably `db/top.cmp0.ddb` / `top.cmp1.ddb` (one per corner). **[inf]**

Corner and file mapping, from matching STA numbers: **[obs]**

- Slow model, speed grade 6: `ddb_magellan4-6.dmf.ddb`. The other DMFs
  (`-7`, `-8`, `-m`, `-mi`) explain at most 4 of the 53 slow-model values.
- Fast model: `ddb_magellan4-m.dmf.ddb` (47/47). `-6`, `-7`, `-8` and `-mi`
  explain at most 6 of the 47.
- `-tt0c` / `-tt85c` (typical, 0 and 85 °C), `-mi` (industrial fast), `-7`,
  `-7a` and `-8` are the other corners and grades. **[inf]** from the names.

## DMF: cell delays (`ddb_<die>-<corner>.dmf.ddb`)

Reader: `tools/ddb/src/dtm.rs`. Implementation: libddb_dtm.so.

`ddb dmf-trace` reproduces the trace's value stream exactly: 84,130 tokens
for `magellan4-6`, including every string, pointer and delta. **[obs]** All
46 `magellan*.dmf.ddb` files parse to the exact end of the body. **[obs]**

### Structure

The body is a tree of `DTM_NODE`s. **[obs]**

```
DTM_NODE                       # all node classes share this layout
  i32 n, n x ptr  m_nodes      # children (new pointers)
  i32  m_id                    # DTM_NODE_ENUM
  bool m_writable
  i32 len + bytes m_description   # exact length, no NUL stored; 0 = none
  i16  m_version               # delta vs previous node
  i16  m_bin                   # delta vs previous node
  ptr  m_data_root             # root of this node's leaf tree, or null
  ptr  m_linked_node           # null in every Cyclone II file
DTM_P2P_NODE : DTM_NODE
  ptr  m_in_out_node, m_in_buried_node, m_buried_out_node, m_buried_buried_node
DTM_TREE_INT_DATA / DTM_TREE_FLOAT_DATA   # an AVL tree of leaves
  ptr  m_left, m_right
  ptr  m_key                   # -> DTM_TREE_DATA_KEY
  i16  m_balance_factor        # delta vs previous leaf
  i32  m_int_data              # delta vs previous INT leaf (floats don't break the chain)
  | f64 m_float_data
DTM_TREE_DATA_KEY
  i32 n, n x DTM_ENUM_WRAPPER { i32 m_enum_type; i32 m_enum_id (delta vs previous wrapper) }
  i16  m_num_wrapper           # delta vs previous key; equals n
```

- The other node classes (`DTM_MICRO_NODE`, `DTM_TABLE_NODE`,
  `DTM_RE_NODE`, ...) add no members. The trace prints them as `DTM_NODE`.
  **[obs]**
- There are three pointer tables: nodes, leaves and keys. Bodies follow
  in that order, each table in id order: first every node, then every leaf,
  then every key. This is the same LIFO drain rule as `asmdb`. The root node
  is the inline top-level object and takes node id 1. **[obs]**
- `m_enum_type` selects the enum of `m_enum_id`, per
  `DTM_ENUM_WRAPPER::get_string` (libddb_dtm.so @ 0x2d65e, a 7-way jump
  table). Types 2-6 store the enum value + 1. **[obs]**

  | type | enum | name table in libdb_dbs.so (64-bit) |
  |---|---|---|
  | 1 | `DTM_ENUM` (parameter names: `TCO`, `CHAIN_BASE`, `LVTTL`, `MIN`, `"7"`, ...) | `DBS_DTM_ENUM_STRING` @ 0x318980, 0x433 entries |
  | 2 | `DB_BURIED_PORT_TYPE` (`FF`, `IO_BUFFER`, ...) | `DB_BURIED_PORT_TYPE_STRING` @ 0x344e80, 0x120 |
  | 3 | `DB_INPUT_PORT_TYPE` (`DATAA`, `CIN`, `CLK`, ...) | `DB_INPUT_PORT_TYPE_STRING` @ 0x31f620, 0x2409 |
  | 4 | `DB_OUTPUT_PORT_TYPE` (`COMBOUT`, `COUT`, `REGOUT`, ...) | `DB_OUTPUT_PORT_TYPE_STRING` @ 0x331680, 0x2700 |
  | 5 | `CDB_RE_TYPE` (`GLOBAL_CLK_H`, `CLK_BUFFER`, ...) | `CDB_RE_TYPE_STRING` @ 0x31b7a0, 0x7cd |
  | 6 | `DEV_IO_STANDARD_ENUM` | `dstr_get_debug_string` |
  | 7 | plain integer (no name; `get_string` falls to its default) | |

  Node ids use `DTM_NODE_ENUM` (`DBS_DTM_NODE_ENUM_STRING` @ 0x31ab20,
  0x18f entries). The table sizes are the bounds checks in
  `DBS_TRANSLATOR_IMPL::get_debug_string_name` (@ 0x9a02e and following).
  The tables are pointer arrays resolved through `R_X86_64_RELATIVE`
  relocations. `tools/ddb/verify/dbs_enum_strings.py` extracts them for
  `ddb dmf <file> <names-dir>`. Like every Quartus-derived table, they stay
  out of the repo. **[obs]**
- Integer leaves are picoseconds for delays. Some leaves hold other units:
  `MAIN_VDD 1100` (mV), `TEMPERATURE -40/0/85`, scale factors in 1/1000
  (`VOLTAGE_DERATING ... SCALE_FACTOR 1202`), `H_LAB_WIDTH 103`. **[obs]** for
  the delays, **[inf]** for the other units.

### Contents (magellan4-6)

6,092 leaves under 445 nodes. Top-level groups: `LE_COMB`, `LE_FF`, `LE`,
`IO` (`VPAD` = column I/O, `HPAD` = row I/O, `CLKPAD`, `OUTPUT_LOAD`),
`CLKBUF`, `CLK_DELAY_CTRL`, `CLK_DELAY_CAL_CTRL`, `EAB/MEAB` (M4K),
`MAC_MULT`, `MAC_OUT`, `PLL`, `IC`, `IC_TABLE`, `RE_INPUT_PORT`,
`TEMPERATURE_DERATING`, `VOLTAGE_DERATING`, `LVDS_RX/TX`, `FLOAT_TABLE`,
`MICRO`. **[obs]** The VPAD/HPAD reading is **[inf]**.

Many groups repeat under `RC_RISE/RC_RISE`, `RC_RISE/RC_FALL`,
`RC_FALL/RC_RISE` and `RC_FALL/RC_FALL` subnodes. These are per-transition
values. The un-split node holds the worst case of the four. **[obs]** that
the un-split value is the maximum in the LE and IO tables. **[inf]** that
the subnodes are transitions.

Values that were checked (slow model, ps): **[obs]**

| DMF leaf | Value | STA report |
|---|---|---|
| `LE_COMB/P2P/InOut` `I:DATAA O:COMBOUT` | 438 | `dataa->combout` 0.438 |
| `LE_COMB/P2P/InOut` `I:DATAB O:COUT CHAIN_BASE` | 393 | `datab->cout` 0.393 |
| `... I:DATAB O:COUT CHAIN_THROUGH_LCB` / `CHAIN_ACROSS_LAB` | 485 / 485 | 0.485 |
| `... I:DATAA O:COUT CHAIN_BASE` | 414 | 0.414 |
| `... I:CIN O:COMBOUT` | 410 | `cin->combout` 0.410 |
| `... I:CIN O:COUT CHAIN_BASE` + `LE_COMB/TABLE INV_CARRY CHAIN_BASE` | 129 − 58 | `cin->cout` 0.071 |
| same, `CHAIN_THROUGH_LCB` (LE 7 → 8) | 230 − 71 | 0.159 |
| same, `CHAIN_ACROSS_LAB` (LE 15 → next LAB) | 219 − 73 | 0.146 |
| `LE_COMB/MICRO REG_FEEDBACK_C_TO_COMBOUT` | 323 | `datac->combout` 0.323 (register fed back into its own LUT) |
| `LE_FF/P2P/InBuried` `I:CLK B:FF` | 537 | `clk` cell 0.537 |
| `LE_FF/P2P/InBuried` `I:DATAIN B:FF` | 84 | `datain` 0.084 |
| `LE_FF/P2P/InBuried` `I:SDATA B:FF` | 366 | `sdata` 0.366 |
| `LE_FF/P2P/BuriedOut` `B:FF O:REGOUT` | 0 | `regout` 0.000 |
| `LE_FF/FLIP-FLOP` `B:FF TCO` / `TSU` / `TH` | 250 / −36 / 266 | uTco 0.250, uTsu 0.036 (= −TSU), uTh 0.266 |
| `IC_TABLE` `RE:CLK_BUFFER RE:CLKBUF_IN 0` | 118 | `CLK_BUFFER` RE 0.118 (CLK0 = PIN_N2) |
| `IC_TABLE` `RE:CLKBUF_OUT RE:GLOBAL_CLK_H 0` / `2` | 12 / 11 | `CLKBUF_OUT` RE 0.012 (G0, blinky) / 0.011 (G2) |
| `IO/HPAD/TABLE` `DELAY_PAD_TO_CORE 6` | 4075 | row-I/O input `IO` RE 4.075 (fitter-chosen input delay chain) |
| fast model, `magellan4-m.dmf`: `TCO` / `TSU` / `TH` | 141 / −32 / 152 | uTco 0.141, uTsu 0.032, uTh 0.152 |

The LUT input → combout delays also depend on a key from `"1"` to
`"15"`, which appears only in the transition subnodes. `DATAA` has the odd
values, `DATAB` the values with bit 1 set, `DATAD` 8-15. So the key is
probably the set of LUT inputs the cell uses (a=1, b=2, c=4, d=8). **[inf]**
Every observed STA value (for example `datab->combout` 0.416, 0.419,
0.420) is one of the leaves for its arc. **[obs]** Which transition subnode
and which mask Quartus picks for a given cell is **[unk]**: the checker
accepts any of them, so it shows that the values come from the DMF, not
the exact lookup rule.

`tools/ddb/verify/dmf_sta_check.py` does this check. On the oracle design
(below), every LE and register value is explained: slow 53/53 distinct
(arc, transition, value) tuples covering 22,513 report rows, and fast 47/47
covering 22,270 rows. **[obs]**

### I/O cells: partly explained

- Input buffer to `combout`: 0.999 ns on dedicated clock pins (N2, N25,
  P25, ...) and 0.832-0.862 ns on row I/O pins. The DMF gives
  `IO/CLKPAD/INPUT_STANDARD/TABLE LVTTL INPUT` = 909 and
  `IO/HPAD/INPUT_STANDARD/TABLE LVTTL INPUT` = 762. The remaining 70-100 ps
  varies per pad in 10 ps steps. **[obs]** Its source is **[unk]**. A
  per-pad or package term in `pad`/`pkg` or `place` is the obvious
  candidate.
- Output buffer (`datain` → pad, 3.3-V LVTTL, default drive): 2.758-2.828 ns
  on column pins. `IO/VPAD/P2P/InOut I:DATAIN O:PADIO` (471) plus
  `IO/VPAD/OUTPUT_STANDARD/TABLE LVTTL TWENTYFOUR` (2151) is 2,622 ps, which
  leaves about 140-200 ps per pad. **[obs]** It may be `IO/OUTPUT_LOAD`
  times a default load, or the same per-pad term as for inputs. **[unk]**

## `timing`: routing element physics (`ddb_<die>_timing.ddb`)

Reader: `tools/ddb/src/rtiming.rs`. `ddb route-timing-trace` reproduces the
first 1,741,109 tokens of the trace exactly. That covers everything up to
and including the packed load-driver array. **[obs]** The deferred
block-type objects after that are not decoded. All six dies parse, and
the element counts equal the routing graph's. **[obs]**

```
ptr (new, class 1 = DYGR_ROUTE_TIMING_INFO_BODY)
i32 n, n x ptr       m_timing_block_types          # 14, deferred
i32 n, n x SIMPLE    m_timing_node_simple_info      # 244 shapes
i32 (delta)          m_num_timing_nodes = 376,788
i32 n, n x i16 (Δ)   m_timing_node_simple_info_indice   # per routing element
i32 n, n x SIMPLE    m_block_interface_simple_info  # 34
i32 (delta)          m_num_block_interface_nodes = 677
i32 n, n x i16 (Δ)   m_block_interface_simple_info_indice
i32 n, n x i32 (Δ)   m_timing_node_load_base_indice # per routing element
i32 (delta)          m_num_loads = 302,783
i32 n, n x { i8 sram1_index; i8 sram2_index; i16 load_position (Δ) }  m_timing_node_load_info
i16 class id + DYGR_PACKED_ARRAY  m_timing_node_load_idriver_info
     bool m_is_simple_array, i16 m_num_bits (2), i32 m_size, i32 m_size_in_bytes,
     i32 n, n x u8 m_array
i32 enum m_current_temperature, DYGR_RC_TEMPERATURE_INFO m_temperature_info   (not decoded)
deferred: DYGR_TIMING_BLOCK_TYPE (m_name "LAB", ..., m_node_map,
          m_node_name_to_int_map), DYGR_ROUTE_TIMING_BLOCK_NODE (m_name "LL:5",
          m_load_nodes "IP:-1C" ..., m_resistance_per_micron,
          m_capacitance_per_micron, m_driver_type, m_sram_idx1/2, m_load_pos, ...)

SIMPLE = DYGR_TIMING_NODE_SIMPLE_INFO
  i16 length, i16 num_drivers, i16 num_avail_temperatures   # deltas vs previous SIMPLE
  i32 n, n x f32 resistance                                  # one per temperature
  i32 n, n x f32 capacitance
  i32 n, n x DYGR_TIMING_NODE_DRIVER_INFO { i32 driver_pos (Δ); i32 driver_type }
```

**[obs]** for the layout. The deltas chain across both SIMPLE vectors.

Element `i` of the routing graph uses `simple_info[indice[i]]`, and its
loads are `load_info[base[i] ..]`, one per fanout edge in fanout order.
**[inf]**: the count isn't stored, but no element's fanout runs past the
list. Many elements share a base, the same way they share routing
templates.

Typical values (magellan4, routing element type from the routing graph,
names from `DEV_ELEMENT_DESC_BODY` in `hdr`, see below): **[obs]** for the
numbers. Units µm, Ω/µm and fF/µm are **[inf]**.

| Element | length | R | C | driver model |
|---|---|---|---|---|
| H4 (R4) | 221-681, mostly 330 | 0.441 | 0.156 | 31 |
| V4 (C4) | 203-833, mostly 758 | 0.465 | 0.223-0.262 | 11 |
| H24 (R24) | 686-2,517 | 0.061 | 0.35-0.36 | 17 |
| V16 (C16) | 555-4,576 | 0.061 | 0.323 | 22 |
| LAB_LINE (rcf `LOCAL_INTERCONNECT`) | 185 (some 40) | 0.563 | 0.236 | 59, 67, 70, 73, 128, 168 |
| LOCAL (STA `LOCAL_LINE`) | 185 | 0.563 | 0.236 | 5 |
| LAB_CLK | 1,495-1,623 | 0.045 | 0.249 | 279 |
| GLOBAL_CLK_V | 3,332 / 3,517 | 0.031 | 0.255 | 270 |
| LE_BUFFER, BLK_CLK_BUF, IO_* | 0 | 0 | 0 | various |
| GLOBAL_CLK_H | 0 | −1 | −1 | 269 |

Every one of the 848 distinct H4/V4/H24/V16/LAB_LINE/LOCAL_LINE/LE_BUFFER
elements in the STA reports joins to a decoded element. STA names
`<RCF type>_X<x>_Y<y>_N0_I<index>` (for example `R4_X42_Y2_N0_I1`) map to
(element enum, x, y, template index). **[obs]** Within one element type the STA
delay grows with length, but spreads widely at equal length. For example,
H4 at length 330 ranges from 0.204 to 0.318 ns. **[obs]** This fits a delay
that depends on the tap position of the next element and on the driver
model, not on the element type alone. **[inf]**

`driver_type` is probably an index into the NLSPC switch models (`sw_seg4h`,
`sw_seg16v`, `sw_local_line_obuff`, ... in `nlspcdata`). **[inf]** The
`DYGR_ROUTE_TIMING_BLOCK_NODE` names (`LL:5`, `IP:12B`) are the LAB-internal
nodes whose names ddb-format.md lists as missing. **[obs]** that the
strings are there. How they map to routing elements is **[unk]**.

## `nlspc`: driver and switch models

Schema from the trace only. Nothing is decoded in Rust. **[obs]** for what
follows unless tagged.

- `ddb_cycloneii-<corner>_nlspcdata.ddb`: `NLSPC_PARSE_INFO_PDB_PLACEHOLDER`,
  257 `NLSPC_PWL2D` (2-D piecewise-linear tables), `NLSPC_SWITCH_MODEL` /
  `t_nlspc_driver_model` (93 of them) with transistor-level parameters
  (`cgd_drive_rise`, `cgs_pass`, `cint_wire`, `coff_fall`, ...), named
  `sw_seg4h`, `sw_seg24h`, `sw_seg16v`, `sw_seg4v_thin/thick`,
  `sw_local_line_obuff`, `sw_lim12`, `sw_vioclk_mux_2sp`, ...
- `ddb_cycloneii-<corner>_nlspccache.ddb`: `NLSPC_CACHE_DATA` →
  `t_nlspc_interp_cache` → `t_nlspc_partgrid` /
  `t_nlspc_reducedwave` (8,336 in `-6`). The cache parameters are named
  `r_total`, `capacitance`, `rise_time`, `driver_position`,
  `measuring_position` and `extra_waveform_points`. So the cache holds
  precomputed waveforms over a grid of total wire resistance, load
  capacitance, input slew and tap position, which the delay calculator
  interpolates. **[inf]**
- This is the step that turns `timing` into routing delays. Reproducing
  it means decoding the cache grids and the interpolation. **[unk]**, and
  the main remaining work.

## `fdi`: the fitter's delay estimates (`ddb_<die>-<corner>_fdi.ddb`)

Schema from the trace (`DYGR_FITTER_DELAY_INFO_BODY`). Not decoded in Rust.
Delays are f32 seconds, as `DYGR_DELAY { rise_delay; fall_delay }`.
`DYGR_MULTI_DELAY` holds two of them, and the unused one is 1e30. **[obs]**

Members, in order: `m_fitter_delay_element_hash` (76),
`m_vector_of_contained_elements` (76 `DEV_ELEMENT_ENUM`), `m_switch_map` /
`m_switch_list` (151 element-pair → delay-id pairs plus
`is_front_delay_lumped`), `m_gid_based_switch_map/list` (768),
`m_rc_based_switch_map/list` (13,157), `m_switch_delay_lookup` (328 ints),
`m_wcid_to_rc_map` (5,293 wire-class → RC product, e.g. 7.67e-12),
`m_elem_pair_switch_type_map/list`, the rise/fall scaling-factor tables,
`m_leim_switch_adjustment_delays_port_to_delay_map`,
`m_lab_output_mode_delta_delays_port_to_delay_map` and `m_use_elmore_for_rc`
(false). Then come 39 `DYGR_FITTER_DELAY_ELEMENT_RC` and 37
`DYGR_FITTER_DELAY_ELEMENT_ID` objects, one per element type. Each has
`m_element_type`, `m_average_linear_delays`, min/max/representative length,
`m_fanout_effect_delays` and `m_wire_position_delays`. RC elements add
`m_set_of_valid_lengths/drivers` and a wire class id per element instance
(`m_vec_of_wire_class_id`, 375,319 in total). ID elements add
`m_vector_of_simple_delays` and `m_gid_to_group_id_map`. The delay-id
indirection is **[unk]**.

The per-type `m_average_linear_delays` against the STA routing-element
delays in the oracle design (slow model): **[obs]**

| Element (STA name) | fdi rise / fall (ps) | STA n | STA min / median / max (ns) |
|---|---|---|---|
| LAB_LINE | 368 / 307 | 261 | 0.166 / 0.362 / 0.397 |
| H4 (`H_SEG4`) | 250 / 199 | 181 | 0.194 / 0.235 / 0.393 |
| LOCAL (`LOCAL_LINE`) | 305 / 275 | 138 | 0.186 / 0.229 / 0.259 |
| LE_BUFFER | 139 / 140 | 132 | 0.050 / 0.056 / 0.076 |
| V4 (`V_SEG4`) | 328 / 264 | 70 | 0.203 / 0.285 / 0.382 |
| H24 (`H_SEG24`) | 472 / 416 | 42 | 0.257 / 0.433 / 0.540 |
| V16 (`V_SEG16`) | 532 / 458 | 24 | 0.312 / 0.465 / 0.664 |
| IO_DATAOUT | 100 / 96 | 27 | 0.091 |
| IO_DATAIN | 64 / 57 | 9 | 0.066 / 0.066 / 0.079 |
| BLK_CLK_BUF | 108 / 118 | 11 | 0.095 / 0.096 / 0.097 |
| LAB_CLK | 362 / 358 | 9 | 0.356 / 0.363 / 0.366 |
| GLOBAL_CLK_V | 166 / 152 | 2 | 0.179 / 0.181 |
| GLOBAL_CLK_H | 390 / 405 | 1 | 0.385 |
| CLK_BUFFER | 116 / 112 | 1 | 0.118 |

The STA also reports `CYCLONEII_LCELL_FF` (0.088, LE register output
stage) and `CYCLONEII_LCELL_COMB` (0-0.025) as routing elements. The fdi
averages fall within ±30% of the STA medians for wires, apart from
LE_BUFFER, where the STA splits the LE output delay differently. They are
usable as a first per-type cost for chipsmith's router. They are not a
substitute for the RC model. **[obs]** for the numbers, **[inf]** for the
advice.

## Element enum names

`DEV_ELEMENT_DESC_BODY` in `ddb_<die>_hdr.ddb` (seen in the fit trace):
`m_int_data[0]` is the `DEV_ELEMENT_ENUM` value, `m_string_data[0]` the
name and `m_string_data[2]` the description. **[obs]** 101 entries for
magellan4. The routing ones:

| enum | name | description | rcf name | STA RE name |
|---|---|---|---|---|
| 43 | LOCAL | Local interconnect | | LOCAL_LINE |
| 45 | LE_BUFFER | | LE_BUFFER | LE_BUFFER |
| 264 | H4 | R4 interconnect | R4 | H_SEG4 |
| 268 | H24 | R24 interconnect | R24 | H_SEG24 |
| 269 | V4 | C4 interconnect | C4 | V_SEG4 |
| 275 | V16 | C16 interconnect | C16 | V_SEG16 |
| 276 | LAB_LINE | Block interconnect | LOCAL_INTERCONNECT | LAB_LINE |
| 289 | CLK_BUFFER | | CLK_BUFFER | CLK_BUFFER |
| 292 / 293 / 294 | GLOBAL_CLK_H / GLOBAL_CLK_V / LAB_CLK | | | same |
| 301 / 311 | IO_DATAIN / IO_DATAOUT | | | same |
| 358 | LAB_SEC_MUX | | | |
| 508 / 509 | LE_COMB / LE_FF | | | |
| 513 / 515 | CLKBUF_IN / CLKBUF_OUT | | | same |
| 532 | LEIM | | | |
| 560 | BLK_CLK_BUF | | | BLK_CLK_BUF |

So the edge-less per-LE element 43 that ddb-format.md guessed at is
`LOCAL`: the LE output feeding back into its own LAB. **[obs]** for the
name, from the STA paths `CYCLONEII_LCELL_FF → LOCAL_LINE → datab`.

## Oracle runs

All Quartus runs used `tools/quartus.sh` (no network). Scratch is in
`/tmp/timing-re/` and is not in the repo.

- **blinky** (`examples/blinky/build/q`, copied): `quartus_fit` and
  `quartus_sta` with `PDB_ASCII_DUMP=on`. The fit trace is 1.07 GB.
- **celltest** (`/tmp/timing-re/celltest/top.v`): a 32-bit counter, 18
  input registers, 64 registers of 4-input XORs, outputs to LEDs and
  pin-to-pin logic, on the DE2 pinout. 128 LEs. SDC: a 20 ns clock and
  0 ns input/output delays on every port. `report_timing -setup` and
  `-hold`, `-npaths 100000 -pairs_only -detail full_path -show_routing`,
  slow and fast models.
- `tools/ddb/verify/sta_rows.py` turns the reports into rows, and
  `dmf_sta_check.py` matches the cell rows against `ddb dmf` output.

## Open questions

- The NLSPC cache grids and interpolation, which is the routing delay
  calculation itself.
- How Quartus picks the DMF transition subnode and LUT-mask key for an
  arc.
- The per-pad 70-200 ps on I/O buffers.
- The deferred `timing` objects (block types and block nodes), the
  temperature info, and `sram1/sram2` (probably which mux level the load
  enters, for the switch model **[inf]**).
- The fdi delay-id indirection (`delay_id_pair` → `m_switch_delay_lookup`).
- The project-side `TIM_RE_DELAY_TREE`. Decoding it would give exact
  per-design routing delays to fit a model against, without parsing STA
  text.
