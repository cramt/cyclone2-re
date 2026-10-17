"""Residual bits of a placed design after routing (ddb) and LUT contents,
expressed in LAB-relative coordinates.

    python3 tools/explore/lab_residual.py <design dir>...

Needs build/top.rcf, build/simulation/modelsim/top.vo and the RBF. Writes
nothing; prints per-LAB residual bits as (frame - lut_frame(x), row offset
in the 70-row LAB band) so LE-level and LAB-level config stand out.
"""
import collections, subprocess, sys
sys.path.insert(0, 'tools/explore')
import vo
from residual import flat_rowcol

EMPTY = 'specimens/route/empty/build/output_files/top.rbf'
CRAM = 'tools/cram/target/release/cram'
RBFDIFF = 'tools/rbfdiff/target/release/rbfdiff'


def lut_frame(x):
    width = lambda c: 166 if c in (13, 26, 52) else 27 if c == 39 else 35
    return 78 + sum(width(c) for c in range(1, x)) + 4 * sum(c <= x for c in (16, 50))


LUT_FRAMES = {lut_frame(x): x for x in range(1, 65) if x not in (13, 26, 39, 52)}


def lab_of(frame, row):
    """(x, y, df, off) of a core CRAM bit, df relative to the nearest LAB
    column's LUT frame; None outside the LAB rows."""
    if row < 18 or row >= 18 + 70 * 35:
        return None
    y = 35 - (row - 18) // 70
    off = (row - 18) % 70
    x = min(LUT_FRAMES.items(), key=lambda kv: abs(frame - kv[0]))[1]
    return x, y, frame - lut_frame(x), off


def le_row(y, le, r):
    """CRAM row of row r (0..3) of LE `le`, LEs 8-15 mirrored."""
    base = 18 + 70 * (35 - y)
    off = 4 * (le % 8) + r
    return base + (off if le < 8 else 69 - off)


def lut_bits(x, y, le, mask):
    """{(frame, row): value} for the 16 LUT entries (stored inverted)."""
    out = {}
    for p in range(16):
        a, b, c, d = p & 1, p >> 1 & 1, p >> 2 & 1, p >> 3 & 1
        out[(lut_frame(x) + 3 - b - 2 * c, le_row(y, le, 3 - a - 2 * d))] = 1 - (mask >> p & 1)
    return out


def observed(d):
    out = subprocess.run([RBFDIFF, '--no-crc', EMPTY, d + '/build/output_files/top.rbf'],
                         capture_output=True, text=True).stdout
    o = {}
    for line in out.splitlines():
        p = line.split()
        if len(p) == 3 and p[1].startswith('f') and 'crc' not in p[1]:
            f, b = map(int, p[1][1:].split(':'))
            o[(f, b)] = int(p[2][-1])
    return o


def routing(d):
    """{(frame, bit): value}: encoded-mux bits take is_encoded_bit_high,
    the rest are 1 when the edge is used."""
    out = subprocess.run([CRAM, 'routing', d], capture_output=True, text=True).stdout
    r = {}
    for l in out.splitlines():
        p = l.split()
        fb = tuple(map(int, p[0][1:].split(':')))
        enc, high = p[1] != 'enc=0', p[2] != 'high=0'
        r[fb] = int(high) if enc else 1
    return r


def load(d):
    """observed bits as {(frame,row): value}, routing set, atoms."""
    obs = {}
    for (f, b), v in observed(d).items():
        if f >= 24:
            obs[(f, flat_rowcol(f, b)[0])] = v
    rt = {(f, flat_rowcol(f, b)[0]): v for (f, b), v in routing(d).items()}
    atoms = vo.parse(d + '/build/simulation/modelsim/top.vo')
    return obs, rt, atoms


def residual(d):
    obs, rt, atoms = load(d)
    lut = {}
    for a in atoms.values():
        if a['kind'] == 'lcell_comb' and a['loc'].startswith('LCCOMB'):
            x, y, le = vo.le_loc(a['loc'])
            lut.update(lut_bits(x, y, le, vo.lut_mask(a)))
    bad_rt = [k for k, v in rt.items() if obs.get(k, 0) != v and not (v == 0 and k not in obs)]
    bad_lut = [k for k, v in lut.items() if obs.get(k, 0) != v]
    res = {k: v for k, v in obs.items() if k not in rt and k not in lut}
    return res, bad_rt, bad_lut, atoms


if __name__ == '__main__':
    for d in sys.argv[1:]:
        res, bad_rt, bad_lut, atoms = residual(d)
        print(f'{d}: residual {len(res)}, routing misses {len(bad_rt)}, lut misses {len(bad_lut)}')
        by = collections.defaultdict(list)
        other = collections.Counter()
        for (f, r), v in sorted(res.items()):
            l = lab_of(f, r)
            if l is None:
                other['io rows'] += 1
            else:
                by[l[:2]].append((l[2], l[3], v))
        print('  outside LAB rows:', dict(other))
        for lab, bits in sorted(by.items()):
            print(f'  LAB {lab}: {len(bits)}', sorted(bits))
