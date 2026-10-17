# Checking `tools/ddb` against Quartus's own trace

1. Put `PDB_ASCII_DUMP=on` in a `quartus.ini` in a compiled project's build
   directory. Run `quartus_fit` (it reads routing) and `quartus_asm` (it
   reads asm) there with stdout redirected into `specimens/pdbdump/`. That
   gives about 1 GB and 5 GB of text.
2. Cut out the section for one ddb: from its `HEADER: m_version_number`
   line to the next one.
3. Compare:

```sh
ddb routing-canon $DEVINFO/ddb_magellan4_routing.ddb > rust.txt
python3 routing_trace_canon.py routing_dump.txt > trace.txt
# m_template also names a member in later objects, so compare the E/T parts
cmp <(head -376788 rust.txt) <(head -376788 trace.txt)
cmp <(grep ^T rust.txt) <(grep ^T trace.txt)

ddb asm-canon $DEVINFO/ddb_magellan4_asm.ddb > rust.txt
python3 asm_trace_canon.py routeasm_dump.txt | sed 's/4294967295/255/g' | cmp - rust.txt
```

For the package and pad databases, the comparison is on the value stream,
and then against the fit reports:

```sh
ddb pins-trace $DEVINFO/ddb_magellan4_pad.ddb | cmp - <(python3 pins_trace_tokens.py pad_dump.txt)
ddb pins-trace $DEVINFO/ddb_magellan4fbga672_pkg.ddb | cmp - <(python3 pins_trace_tokens.py pkg_dump.txt)
ddb pins $DEVINFO magellan4 fbga672 > pins.csv
python3 pins_fit_check.py pins.csv $(find specimens -name '*.fit.rpt')
```

The block configuration tables (`asmdb`) are in the `quartus_asm` trace,
between its 9th and 10th `HEADER: m_version_number` (the asmdb section,
about 288k lines). Mux bit lists in id order, arch group headers and block
instance lists must match exactly (17,290 muxes, 529 arch groups, 4 block
types for magellan4):

```sh
ddb asmdb-canon $DEVINFO/ddb_magellan4_asmdb.ddb > rust.txt
python3 asmdb_canon.py asmdb_trace.txt > trace.canon
python3 asmdb_semantic.py trace.canon | cmp - rust.txt
```

The trace is Quartus-derived, so it never leaves `specimens/`.
