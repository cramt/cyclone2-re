#!/usr/bin/env python3
"""Canonical form of the asmdb body from a PDB_ASCII_DUMP trace, to diff
against `ddb asmdb-canon`.

    asmdb_canon.py <asmdb section of the quartus_asm trace> > trace.canon

One line per serialized object body (`CLASS v v v ...`), every numeric
value as the reader decoded it (absolute, not the delta), signed decimal.
Strings are written as s:<text>. Enum keys, which the trace prints twice,
appear once.
"""
import re, sys

out = []
cur = None
started = False
for l in open(sys.argv[1], encoding='latin1'):
    l = l.rstrip('\n')
    if not started:
        started = l.startswith('FFFFFFFF') and l.rstrip().endswith('ptr')
        continue
    m = re.match(r'^ASMDB_(\w+?)\s*ptr\s*$', l)
    if m:
        cur = [m.group(1)]
        out.append(cur)
        continue
    if cur is None or '(BASE CLASS)' in l or not l.strip():
        continue
    head = l[:24]
    tok = head.split()
    if not tok:
        continue
    v = tok[0]
    if len(tok) > 1 and tok[1] == 'enum':
        continue
    if v.startswith('"'):
        cur.append('s:' + l.strip().strip('"').replace('\\0', ''))
    elif re.fullmatch(r'[0-9A-F]{2}|[0-9A-F]{4}|[0-9A-F]{8}', v):
        bits = 4 * len(v)
        x = int(v, 16)
        if x >= 1 << (bits - 1):
            x -= 1 << bits
        cur.append(str(x))
    elif v.startswith("'"):
        cur.append('c:' + l[1:l.index("'", 1)] if "'" in l[1:] else 'c:')
for o in out:
    print(' '.join(o))
