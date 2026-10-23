import sys
# Value stream of one DMF (DTM delay model) section of a PDB_ASCII_DUMP, one
# token per line, in the form `ddb dmf-trace` prints: hex ints at the width
# the trace uses (8 for i32, 4 for i16, 2 for bool), %E floats, "quoted"
# c_strs, and "@CLASS" where an object body starts.
out = sys.stdout
started = False
for ln in open(sys.argv[1], 'rb'):
    if not started:
        started = ln.split()[:1] == [b'FFFFFFFF']
        if started:
            out.write('FFFFFFFF\n')
        continue
    if ln.startswith(b'"'):
        out.write('"' + ln.rstrip(b'\n')[1:-3].decode('latin-1') + '"\n')
        continue
    f = ln.split()
    if not f or b'HEADER:' in f or b'FOOTER:' in f:
        continue
    if f[-1] == b'ptr' and f[0].startswith(b'DTM_'):
        out.write('@' + f[0].decode() + '\n')
        continue
    # "(BASE CLASS)" markers, wrapper headers, and the second copy of enums.
    if f[0].startswith(b'DTM_') or (len(f) > 1 and f[1] == b'enum'):
        continue
    out.write(f[0].decode() + '\n')
