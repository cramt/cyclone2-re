"""Tabulate LE-level asmdb groups against netlist facts, to learn setting
semantics.

    python3 tools/explore/le_facts.py <design dir>...

For every LE (x, y, le) the design uses: facts from the post-fit netlist
and rcf (LUT used, cin used, cout used, register used, which control ports
are live, what drives each LE output), and the observed bit of every
one-bit LE group (LE_USE_CARRY, LE_CLOCK_SELECT, ...). Prints a table of
(fact tuple -> bit values) per group.
"""
import collections, re, sys
sys.path.insert(0, 'tools/explore')
import lab_residual as L
import lab_model as M
import vo
import rcfpath as R

ONE_BIT = ['LE_USE_CARRY', 'LE_CLOCK_SELECT', 'LE_CLEAR_SELECT', 'LE_USE_REG_CASCADE_ARM',
           'LE_USE_REGISTER_FEEDBACK', 'LE_USE_SYNC_CLEAR', 'LE_REG_OUT_YGR']


def group(name, gi=None):
    return [g for g in M.GROUPS if g[0] == name and (gi is None or g[1] == gi)]


def le_facts(d):
    obs, rt, atoms = L.load(d)
    les = collections.defaultdict(dict)
    for n, a in atoms.items():
        if not a['loc'].startswith('LC'):
            continue
        x, y, le = vo.le_loc(a['loc'])
        les[(x, y, le)]['comb' if a['kind'] == 'lcell_comb' else 'ff'] = (n, a)
    # Who drives what: rcf LE_BUFFER/LOCAL_LINE resources start a net.
    drivers = collections.defaultdict(set)
    for sig, items in R.parse_rcf(d + '/build/top.rcf'):
        for k, v in items:
            if k == 'res' and v[0] in ('LE_BUFFER', 'LOCAL_LINE'):
                drivers[(v[1], v[2], v[0], v[4])].add(sig)
    out = []
    for (x, y, le), c in sorted(les.items()):
        f = {}
        comb = c.get('comb')
        ff = c.get('ff')
        f['lut'] = comb is not None
        if comb:
            p = comb[1]['ports']
            f['lutc'] = comb[1]['params'].get('sum_lutc_input')
            f['cin'] = p.get('cin') not in (None, 'gnd', 'vcc')
            f['cout'] = 'cout' in p
            f['fb'] = ff is not None and ff[1]['ports'].get('regout') in [p.get(k) for k in ('dataa', 'datab', 'datac', 'datad')]
        f['ff'] = ff is not None
        if ff:
            p = ff[1]['ports']
            f['clk'] = p.get('clk')
            for k in ('ena', 'aclr', 'sclr', 'sload'):
                f[k] = p.get(k) not in (None, 'gnd', 'vcc')
            f['din_lut'] = comb is not None and p.get('datain') == comb[1]['ports'].get('combout')
        combout = comb[1]['ports'].get('combout') if comb else None
        regout = ff[1]['ports'].get('regout') if ff else None
        for o, (kind, idx) in enumerate([('LE_BUFFER', 2 * le), ('LE_BUFFER', 2 * le + 1), ('LOCAL_LINE', le)]):
            sigs = drivers.get((x, y, kind, idx), set())
            f[f'out{o}'] = 'comb' if combout in sigs else 'reg' if regout in sigs else '-'
        base = M.lab_base(x, y)
        bits = {}
        for name, gi, paths, aux, nxt in M.GROUPS:
            if name not in ONE_BIT or len(paths) < 16:
                continue
            p = paths[le] if name != 'LE_REG_OUT_YGR' else paths[le]
            vals = ''.join(str(obs.get(M.fr(base + b), '.')) if b >= 0 else 'x' for lvl in p for b in lvl)
            bits[f'{name}[{gi}]' if gi >= 0 else name] = vals
        out.append(((x, y, le), f, bits))
    return out


if __name__ == '__main__':
    rows = [r for d in sys.argv[1:] for r in le_facts(d)]
    for (xyz, f, bits) in rows:
        print(xyz, ' '.join(f'{k}={v}' for k, v in f.items()), '||', ' '.join(f'{k}={v}' for k, v in bits.items()))
