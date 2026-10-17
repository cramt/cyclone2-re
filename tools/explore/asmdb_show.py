"""Pretty-print asmdb arch groups (after asmdb_trace.py).

    python3 tools/explore/asmdb_show.py /tmp/asmdb.pkl <type name or number>...
"""
import json, pickle, re, sys

bodies, ids, refs = pickle.load(open(sys.argv[1], 'rb'))
NAMES = json.load(open('/tmp/asmx/archtype.json'))
by_id = {t_i: b for t_i, b in zip(ids, bodies)}
refs_by_body = {}
for bi, member, t, ident in refs:
    refs_by_body.setdefault(bi, []).append((member, t, ident))
body_index = {t_i: i for i, t_i in enumerate(ids)}


def ints(fields, name):
    out = []
    for member, depth, val in fields:
        if re.sub(r'\[\d+\]', '', member) == name and '[' in member and re.fullmatch(r'[0-9A-F]{8}', val):
            v = int(val, 16)
            out.append(v - (1 << 32) if v >= 1 << 31 else v)
    return out


def mux(ident, depth=0):
    cls, fields = by_id[('mux', ident)]
    bi = body_index[('mux', ident)]
    bits = ints(fields, 'm_mux_bits')
    sub = [(m, i) for m, t, i in refs_by_body.get(bi, []) if t == 'mux']
    name = [v for m, d, v in fields if m == 'm_name']
    s = f"{'  ' * depth}mux#{ident} {cls} bits={bits[:40]}{'...' if len(bits) > 40 else ''} n={len(bits)}"
    for m, i in sub:
        s += f"\n{'  ' * depth}  {m} -> " + (mux(i, depth + 2).lstrip() if i and depth < 4 else str(i))
    return s


def show(ai):
    cls, fields = by_id[('arch', ai)]
    t = [int(v, 16) for m, d, v in fields if m == 'm_type' and re.fullmatch(r'[0-9A-F]{8}', v)][0]
    bt = [int(v, 16) for m, d, v in fields if m == 'm_group_block_type' and re.fullmatch(r'[0-9A-F]{8}', v)][0]
    print(f'arch#{ai} {cls} type={t} {NAMES[t]} block={bt}')
    for m, d, v in fields:
        if m in ('m_group_index', 'arch_value') or m.startswith('m_bits') or m.startswith('m_default'):
            print('   ', m, v)
    bi = body_index[('arch', ai)]
    for m, tb, i in refs_by_body.get(bi, []):
        if tb == 'mux' and i:
            print('  ', m, '->', mux(i, 2).lstrip())
        else:
            print('  ', m, '->', tb, i)


want = sys.argv[2:]
for (t, i), (cls, fields) in zip(ids, bodies):
    if t != 'arch':
        continue
    ty = [int(v, 16) for m, d, v in fields if m == 'm_type' and re.fullmatch(r'[0-9A-F]{8}', v)][0]
    if NAMES[ty] in want or str(ty) in want:
        show(i)


def rc(b):
    """Block-relative flat bit -> (row within block, frame - lut_frame) for
    a LAB (block base column = lut_frame + 12)."""
    return (b // 2701, 12 - b % 2701)
