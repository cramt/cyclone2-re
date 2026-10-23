import sys
# Value stream of the ddb_<die>_timing.ddb section of a PDB_ASCII_DUMP, in
# the form `ddb route-timing-trace` prints (hex ints at the trace's width,
# %E floats). Object headers, "(BASE CLASS)" lines and the second copy of
# enums are dropped. The Rust decoder stops after the packed load-driver
# array, so compare its output against the same number of leading tokens.
out = sys.stdout
started = False
for ln in open(sys.argv[1], 'rb'):
    if not started:
        started = ln.split()[:1] == [b'FFFFFFFF']
        if started:
            out.write('FFFFFFFF\n')
        continue
    if ln.startswith(b'"') or ln.startswith(b"'"):
        continue
    f = ln.split()
    if not f or b'HEADER:' in f or b'FOOTER:' in f:
        continue
    if f[0].startswith(b'DYGR_') or (len(f) > 1 and f[1] == b'enum'):
        continue
    out.write(f[0].decode() + '\n')
