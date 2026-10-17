"""Resolve .rcf routing resources to magellan4 routing-graph edges.

Exploration helper for the CRAM address fit; the Rust implementation is the
deliverable. An rcf resource `C4:X63Y15S0I12` is (type, x, y, z=S, index=I).
The type -> element_enum table was found by matching rcf chains against graph
edges: every consecutive pair resolved to exactly one edge.
"""
import collections, csv, os, pickle, re

P = 'specimens/db/magellan4/'
TYPE_ENUM = {
    'LE_BUFFER': 45, 'LOCAL_INTERCONNECT': 276, 'R4': 264, 'R24': 268,
    'C4': 269, 'C16': 275, 'CLK_BUFFER': 289,
}


def load():
    pk = '/tmp/m4.pkl'
    if os.path.exists(pk):
        return pickle.load(open(pk, 'rb'))
    raise SystemExit('run tools/explore/load.py first')


RES = re.compile(r'^\s*([A-Z0-9_]+):X(\d+)Y(\d+)S(\d+)I(\d+);')
DEST = re.compile(r'dest = \( (.*?), (\w+) \)(?:, route_port = (\w+))?;')


def parse_rcf(path):
    """[(signal, [(type,x,y,z,i)...], [dest...])] where a branch restarts at
    a resource already seen in the signal (rcf writes trees depth-first)."""
    sigs = []
    cur = None
    for line in open(path):
        if line.startswith('signal_name'):
            cur = (line.split('=')[1].strip(' {\n'), [])
            sigs.append(cur)
            continue
        m = RES.match(line)
        if m and cur:
            t, x, y, z, i = m.groups()
            cur[1].append(('res', (t, int(x), int(y), int(z), int(i))))
            continue
        m = DEST.search(line)
        if m and cur:
            cur[1].append(('dest', m.groups()))
        if line.strip() == 'branch_point = 0;' and cur:
            cur[1].append(('branch', None))
    return sigs


def resolve(sig_items, nodes, byloc, out):
    """Walk one signal's resources; return [(src, dst, bits)] for each hop
    between consecutive resources, plus the trailing LOCAL_INTERCONNECT /
    IO nodes whose sink edge we can't name yet."""
    hops, prev, unresolved = [], None, []
    for kind, val in sig_items:
        if kind != 'res':
            continue
        t, x, y, z, i = val
        cands = byloc.get((x, y, z, i), [])
        if prev is None:
            e = TYPE_ENUM.get(t)
            cs = [n for n in cands if nodes[n][4] == e]
            prev = cs[0] if len(cs) == 1 else None
            if prev is None:
                unresolved.append(val)
            continue
        nxt = [n for n in cands if n in out.get(prev, {})]
        if len(nxt) != 1:
            unresolved.append((val, len(nxt)))
            prev = None
            continue
        d = nxt[0]
        hops.append((prev, d, out[prev][d][1]))
        prev = d
    return hops, unresolved, prev


def observed(diff_path):
    """{(frame, bit): new_value} for data bits from `rbfdiff --no-crc`."""
    o = {}
    for line in open(diff_path):
        p = line.split()
        if len(p) == 3 and p[1].startswith('f'):
            f, b = p[1][1:].split(':')
            o[(int(f), int(b))] = int(p[2][-1])
    return o
