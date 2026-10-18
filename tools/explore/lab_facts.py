"""Per-LAB control facts vs the full value of every LAB-wide asmdb group.

    python3 tools/explore/lab_facts.py <design dir>...

Facts come from the rcf: which nets reach which LAB control mux line
(LAB_CONTROL_MUX index, i.e. LAB_PIA_TO_INPUT k) and register port (CLK,
ENA, ACLR, SCLR, SLOAD), and global clocks by BLK_CLK_BUF index; the .vo
says whether a clock is inverted. Group values are printed whole (empty
design's value where unchanged) so encoded settings can be read off.
"""
import collections, re, subprocess, sys
sys.path.insert(0, 'tools/explore')
import lab_residual as L
import lab_model as M
import rcfpath as R
import vo
from residual import flat_rowcol

EMPTY_RBF = 'specimens/route/empty/build/output_files/top.rbf'


def rbf_bits(path):
    """(frame, row) -> bit for every core bit (frames >= 24)."""
    data = open(path, 'rb').read()
    def get(frame, row):
        lane, pos = (7, row - 2176) if row >= 2176 else divmod(row, 311)
        byte = 311 - pos
        off = 0x186 + frame * 314 + byte
        return data[off] >> lane & 1
    return get


EMPTY = rbf_bits(EMPTY_RBF)
LAB_GROUPS = [g for g in M.GROUPS if g[0].startswith('LAB_')]


def facts(d):
    atoms = vo.parse(d + '/build/simulation/modelsim/top.vo')
    # rcf dests name a cell by its output node (q66), the .vo by instance
    # (f67): accept both.
    cell_le, cell_atom = {}, {}
    for n, a in atoms.items():
        if a['loc'].startswith('LC'):
            for k in (n, a['ports'].get('regout'), a['ports'].get('combout')):
                if k:
                    cell_le[k] = vo.le_loc(a['loc'])
                    cell_atom[k] = a
    labs = collections.defaultdict(lambda: {'pia': {}, 'ctl': collections.defaultdict(set), 'clk': {}})
    for sig, items in R.parse_rcf(d + '/build/top.rcf'):
        prev = None
        last = {}
        for k, v in items:
            if k == 'res':
                if v[0] == 'LAB_CONTROL_MUX' and prev and prev[0] == 'LOCAL_INTERCONNECT':
                    labs[(v[1], v[2])]['pia'][v[4]] = (sig, prev[4])
                prev = v
                last = v
            elif k == 'dest':
                cell, port, rport = v
                if cell not in cell_le:
                    continue
                x, y, le = cell_le[cell]
                if last and last[0] == 'LAB_CONTROL_MUX':
                    labs[(x, y)]['ctl'][(port, last[4])].add(sig)
                elif last and last[0] == 'BLK_CLK_BUF':
                    inv = cell_atom[cell]['ports'].get(port.lower(), '').startswith('!')
                    labs[(x, y)]['clk'][(port, last[4], inv)] = sig
    return labs


def values(d, x, y):
    obs, rt, atoms = L.load(d)
    base = M.lab_base(x, y)
    out = {}
    for name, gi, paths, aux, nxt in LAB_GROUPS:
        for pi, p in enumerate(paths):
            s = []
            for lvl in p:
                for b in lvl:
                    if b < 0:
                        s.append('x')
                        continue
                    k = M.fr(base + b)
                    s.append(str(obs.get(k, EMPTY(*k))))
                s.append('|')
            out[f'{name}[{gi}]'] = ''.join(s[:-1])
    return out


if __name__ == '__main__':
    for d in sys.argv[1:]:
        print('==', d)
        for (x, y), f in sorted(facts(d).items()):
            ctl = {k: sorted(v) for k, v in f["ctl"].items() if not k[0].startswith('DATA')}
            print(f'LAB X{x} Y{y} glob={dict(f["clk"])} pia={f["pia"]} ctl={ctl}')
            vals = values(d, x, y)
            short = {'LAB_CE_FAST_MUX': 'CE', 'LAB_CLOCK_FAST_MUX': 'CK', 'LAB_INVERT': 'INV', 'LAB_CLR_INVERT_PAIR': 'CIP',
                     'LAB_ENABLE_CE': 'ECE', 'LAB_ENABLE_SYNC_SIGNALS': 'SYN', 'LAB_SYNC_CLR_FAST_MUX': 'SCM',
                     'LAB_SYNC_LOAD_FAST_MUX_MAG': 'SLM', 'LAB_CLEAR_FAST_MUX': 'CLR'}
            print('     ' + ' '.join(f"{short[k.split('[')[0]]}{k[k.index('['):].replace('[-1]','').replace('[6]','')}={v}"
                                     for k, v in vals.items() if 'PIA_TO_INPUT' not in k))
