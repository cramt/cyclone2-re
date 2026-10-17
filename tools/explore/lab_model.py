"""LAB configuration bits named by the asmdb, read back from fuzz designs.

    python3 tools/explore/lab_model.py <design dir>...

For each LAB a design uses, prints every LAB-block arch group whose bits
differ from the empty design, per path (per LE for LE-level groups), with
the bit values: the raw material for the setting semantics.
"""
import collections, pickle, re, sys
sys.path.insert(0, 'tools/explore')
import lab_residual as L

bodies, ids, refs = pickle.load(open('/tmp/asmdb.pkl', 'rb'))
import json
NAMES = json.load(open('/tmp/asmx/archtype.json'))
by_id = {t_i: b for t_i, b in zip(ids, bodies)}
body_index = {t_i: i for i, t_i in enumerate(ids)}
refs_by_body = collections.defaultdict(list)
for bi, member, t, ident in refs:
    refs_by_body[bi].append((member, t, ident))


def ints(fields, name):
    out = []
    for member, depth, val in fields:
        if re.sub(r'\[\d+\]', '', member) == name and '[' in member and re.fullmatch(r'[0-9A-F]{8}', val):
            v = int(val, 16)
            out.append(v - (1 << 32) if v >= 1 << 31 else v)
    return out


def mux_bits(i):
    return ints(by_id[('mux', i)][1], 'm_mux_bits')


GROUPS = []   # (name, gi, [[mux bits per level] per path], aux, next)
for (t, i), (cls, fields) in zip(ids, bodies):
    if t != 'arch':
        continue
    ty = [int(v, 16) for m, d, v in fields if m == 'm_type' and re.fullmatch(r'[0-9A-F]{8}', v)][0]
    bt = [int(v, 16) for m, d, v in fields if m == 'm_group_block_type' and re.fullmatch(r'[0-9A-F]{8}', v)][0]
    if bt != 18:
        continue
    gi = [m for m in fields if m[0] == 'm_group_index'][0][2]
    r = refs_by_body[body_index[('arch', i)]]
    paths = []
    for m, tt, x in r:
        if m.startswith('m_mux_path_list') and x:
            if m.endswith('[0]') and (not paths or True):
                pass
    # Rebuild the nested path structure from the field order.
    plist, cur = [], None
    it = iter([x for m, tt, x in r if m.startswith('m_mux_path_list')])
    for m, d, v in fields:
        if m.startswith('m_mux_path_list') and d == 1:
            cur = []
            plist.append(cur)
        elif m.startswith('m_mux_path_list') and d == 2:
            cur.append(next(it))
    aux = [x for m, tt, x in r if m == 'aux_mux']
    nxt = [x for m, tt, x in r if m == 'next_mux_selection']
    GROUPS.append((NAMES[ty], int(gi, 16) if gi != 'FFFFFFFF' else -1,
                   [[mux_bits(x) if x else [] for x in p] for p in plist],
                   mux_bits(aux[0]) if aux and aux[0] else None,
                   mux_bits(nxt[0]) if nxt and nxt[0] else None))


def lab_base(x, y):
    """Flat address of LAB (x, y)'s block base: row = first LAB row,
    column = lut_frame + 12 (asmdb ASMDB_BLOCK_INFO, observed)."""
    row = 18 + 70 * (35 - y)
    col = 2724 - (L.lut_frame(x) + 12)
    return row * 2701 + col


def fr(flat):
    return 2724 - flat % 2701, flat // 2701


def run(d):
    obs, rt, atoms = L.load(d)
    labs = sorted({vo_xy(a['loc']) for a in atoms.values() if a['loc'].startswith('LC')})
    for (x, y) in labs:
        base = lab_base(x, y)
        print(f'LAB X{x} Y{y}')
        for name, gi, paths, aux, nxt in GROUPS:
            if name in ('LE_LUT_FUNCTION',):
                continue
            for pi, p in enumerate(paths):
                vals = [[obs.get(fr(base + b), '.') if b >= 0 else 'x' for b in lvl] for lvl in p]
                if any(v != '.' for lvl in vals for v in lvl):
                    print(f'   {name}[{gi}] path {pi}: ' + ' | '.join(''.join(map(str, l)) for l in vals))


def vo_xy(loc):
    m = re.match(r'LC(?:COMB|FF)_X(\d+)_Y(\d+)', loc)
    return int(m.group(1)), int(m.group(2))


if __name__ == '__main__':
    for d in sys.argv[1:]:
        print('==', d)
        run(d)
