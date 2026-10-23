"""Every M4K asmdb group's bits in one block of an RBF, against a base RBF.

    python3 tools/explore/m4k_state.py <asmdb.json> <archtype.json> <base.rbf> <x> <y> <rbf>...

asmdb.json is `ddb asmdb-json` output, archtype.json the arch type name
table. For each group prints the bits that differ from the base; for input
muxes also the decoded source (two-level: first-level bit = aux[source],
second-level bit = next[source]). Content bits (RAM_CONTENT_YGR_MEAB) are
summarised.
"""
import json, sys

asm = json.load(open(sys.argv[1]))
NAMES = json.load(open(sys.argv[2]))
M = asm['muxes']
COLS = 2701
M4K_COLS = {13: 2083, 26: 1493, 52: 456}


def frame_bit(flat):
    row, col = divmod(flat, COLS)
    lane, pos = (7, row - 2176) if row >= 2176 else divmod(row, 311)
    return 2724 - col, (311 - pos) * 8 + lane


def reader(path):
    data = open(path, 'rb').read()
    def get(flat):
        f, b = frame_bit(flat)
        return data[0x186 + f * 314 + b // 8] >> (b % 8) & 1
    return get


def base_of(x, y):
    return (18 + 70 * (35 - y)) * COLS + M4K_COLS[x]


def groups():
    for g in asm['groups']:
        if g['block'] == 39:
            yield g


def main():
    base_rbf = reader(sys.argv[3])
    x, y = int(sys.argv[4]), int(sys.argv[5])
    b0 = base_of(x, y)
    for path in sys.argv[6:]:
        get = reader(path)
        print(f'== {path}')
        for g in groups():
            name = f"{NAMES[g['type']]}[{g['index']}]"
            levels = [M[m]['bits'] if m is not None else [] for m in g['body']['paths'][0]]
            vals = [[get(b0 + o) if o >= 0 else None for o in lv] for lv in levels]
            bvals = [[base_rbf(b0 + o) if o >= 0 else None for o in lv] for lv in levels]
            if 'CONTENT' in name:
                ones = sum(v for v in vals[0] if v)
                print(f'{name}: {ones} of {len(vals[0])} set')
                continue
            if vals == bvals and '-a' not in sys.argv[0:1]:
                continue
            s = ' '.join(''.join('.' if v is None else str(v) for v in lv) for lv in vals)
            sb = ' '.join(''.join('.' if v is None else str(v) for v in lv) for lv in bvals)
            extra = ''
            d = g['body']['data'][0]
            if 'INPUT' in name and d[1] is not None:
                aux, nxt = M[d[1]]['bits'], M[d[2]]['bits']
                l0 = [i for i, v in enumerate(vals[0]) if v]
                l1 = [i for i, v in enumerate(vals[1]) if v] if len(vals) > 1 else []
                srcs = [s_ for s_ in range(len(aux)) if aux[s_] in l0 and (not l1 or nxt[s_] in l1)]
                extra = f' sources {srcs}'
            print(f'{name}: {s}  (base {sb}){extra}')


main()
