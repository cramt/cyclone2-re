import sys
# Value stream of one pad/pkg db section of a PDB_ASCII_DUMP, one token per
# line, in the form `ddb pins-trace` prints: 8-digit hex ints, %E doubles,
# "quoted" strings, and "@CLASS" where a deferred object body starts.
out = sys.stdout
# The header/footer strings come first; the body starts at the top-level ptr.
started = False
for ln in open(sys.argv[1], 'rb'):
    if not started:
        started = ln.split()[:1] == [b'FFFFFFFF']
        if not started:
            continue
    if ln.startswith(b'"'):
        out.write('"' + ln.rstrip(b'\n')[1:-3].decode('latin-1') + '"\n')
        continue
    f = ln.split()
    if not f or b'HEADER:' in f or b'FOOTER:' in f:
        continue
    if f[-1].endswith(b'ptr') and not f[0][:1].isdigit() and not f[0].startswith(b'FFFFFFFF'):
        out.write('@' + f[0].decode().removesuffix('ptr') + '\n')
        continue
    out.write(f[0].decode() + '\n')
