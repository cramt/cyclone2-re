import os, struct, subprocess, sys
# Extract the enum name tables the DMF keys use from Quartus's libdb_dbs.so
# (64-bit) into <out>/<table>.txt, one "<value> <name>" per line, for
# `ddb dmf <file> <out>`. The tables are pointer arrays resolved through
# R_X86_64_RELATIVE relocations; the bounds are the checks in
# DBS_TRANSLATOR_IMPL::get_debug_string_name.
#   python3 dbs_enum_strings.py ~/altera/13.0sp1/quartus/linux64/libdb_dbs.so out/
lib, outdir = sys.argv[1], sys.argv[2]
TABLES = {  # file: (symbol, count)
    'dtm_enum': ('DBS_DTM_ENUM_STRING', 0x433),
    'dtm_node_enum': ('DBS_DTM_NODE_ENUM_STRING', 0x18f),
    'cdb_re_type': ('CDB_RE_TYPE_STRING', 0x7cd),
    'db_input_port': ('DB_INPUT_PORT_TYPE_STRING', 0x2409),
    'db_output_port': ('DB_OUTPUT_PORT_TYPE_STRING', 0x2700),
    'db_buried_port': ('DB_BURIED_PORT_TYPE_STRING', 0x120),
}
def run(*a):
    return subprocess.run(a, capture_output=True, text=True, check=True).stdout
syms = {}
for l in run('nm', '-D', '--defined-only', lib).splitlines():
    t = l.split()
    if len(t) == 3:
        syms[t[2]] = int(t[0], 16)
# DBS_DTM_*_STRING are local; take them from the full symbol table if present,
# else from the lea in get_debug_string_name (see docs/timing-model.md).
for l in run('nm', lib).splitlines():
    t = l.split()
    if len(t) == 3:
        syms.setdefault(t[2], int(t[0], 16))
rel = {}
for l in run('readelf', '-rW', lib).splitlines():
    t = l.split()
    if len(t) >= 4 and t[2] == 'R_X86_64_RELATIVE':
        rel[int(t[0], 16)] = int(t[3], 16)
segs = []
for l in run('readelf', '-lW', lib).splitlines():
    t = l.split()
    if t and t[0] == 'LOAD':
        segs.append((int(t[2], 16), int(t[1], 16), int(t[4], 16)))
data = open(lib, 'rb').read()
def cstr(va):
    for v, o, fs in segs:
        if v <= va < v + fs:
            p = o + va - v
            return data[p:data.index(b'\0', p)].decode('latin-1')
os.makedirs(outdir, exist_ok=True)
for name, (sym, n) in TABLES.items():
    base = syms[sym]
    lines = []
    for i in range(n):
        t = rel.get(base + 8 * i)
        lines.append(f'{i} {cstr(t) if t else ""}')
    open(os.path.join(outdir, name + '.txt'), 'w').write('\n'.join(lines) + '\n')
    print(name, len(lines))
