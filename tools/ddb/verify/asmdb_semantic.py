#!/usr/bin/env python3
"""Turn asmdb_canon.py's trace canon into the summary `ddb asmdb-canon`
prints (mux bit lists in id order, arch groups, block infos), for diffing.

    asmdb_semantic.py trace.canon > trace.summary
"""
import sys

MUX = {'LABELED_SIMPLE_MUX', 'MAPPED_SIMPLE_MUX', 'MAPPED_ENCODED_MUX', 'STL_MAPPED_ENCODED_MUX', 'MUX',
       'DECISION_MUX', 'ENCODED_BANK_MUX', 'RAM_MUX', 'SIMPLE_MUX', 'ENCODED_MUX'}
muxes, arch, blocks, lists = [], [], [], []
for line in open(sys.argv[1]):
    cls, *v = line.split()
    if cls in MUX:
        n = int(v[0])
        muxes.append('M' + ''.join(' ' + x for x in v[1:1 + n]))
    elif cls.startswith('ARCH_GROUP'):
        gi, ty, bt = v[0], v[1], v[2]
        paths = []
        if cls != 'ARCH_GROUP_DUMB':
            n, i = int(v[3]), 4
            for _ in range(n):
                k = int(v[i])
                paths.append(k)
                i += 1 + k
        arch.append(f'A {ty} {bt} {gi} [{", ".join(map(str, paths))}]')
    elif cls == 'BLOCK_INFO':
        # bcm, two null strings, map count, key, presence flag, n, elems...
        i = 3
        nmap = int(v[i]); i += 1
        total = 0
        elems = []
        for _ in range(nmap):
            i += 1                      # key
            if v[i].startswith('c:'):   # presence flag printed as a char
                i += 1
            n = int(v[i])
            elems += v[i + 1:i + 1 + n]
            i += 1 + n
            total += n
        bt = v[i]; cols = v[i + 1]
        ns = int(v[i + 2]); kv = v[i + 3:i + 3 + 2 * ns]
        d = '{' + ', '.join(f'{kv[j]}: {kv[j + 1]}' for j in range(0, len(kv), 2)) + '}'
        blocks.append(f'B {bt} {cols} {total} {d}')
        lists.append('L ' + ' '.join(elems))
print('\n'.join(muxes + arch + blocks + lists))
