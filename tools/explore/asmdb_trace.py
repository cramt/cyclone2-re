"""Object graph of the Cyclone II `asmdb` (block config mux -> bit tables),
rebuilt from Quartus's own PDB_ASCII_DUMP trace of quartus_asm.

    python3 tools/explore/asmdb_trace.py specimens/pdbdump/asmdb_trace.txt out.pkl

Exploration only: the trace is Quartus-derived and stays in specimens/.
The point is to learn the structure well enough to write a real decoder
(tools/ddb) and to name the LE/LAB bits that fuzzing finds.

Trace lines are `<value> [<delta>] <tabs><member>`; a deferred object body
starts with `<CLASS> ptr`. Pointers: 0 null, negative = new object of
footer class -v (takes the next id of its pointer table), positive =
back-reference, id = (last id resolved in that table + v) & 0x3fffffff.
Bodies are written per table in id order, so the k-th body of a table is
id k.
"""
import pickle, re, sys

CLASS = {1: 'DIE', 2: 'BLOCK_INFO', 3: 'ARCH_GROUP_CALC', 4: 'ARCH_GROUP_STD', 5: 'ARCH_GROUP_DUMB',
         6: 'ARCH_GROUP_STLMAP', 7: 'DEFAULT_MUX_SETTING', 8: 'DEFAULT_BIT_CALC', 9: 'LABELED_SIMPLE_MUX',
         0xa: 'MAPPED_SIMPLE_MUX', 0xb: 'SIMPLE_MUX', 0xc: 'ENCODED_MUX', 0xd: 'DECISION_MUX',
         0xe: 'MAPPED_ENCODED_MUX', 0xf: 'STL_MAPPED_ENCODED_MUX', 0x10: 'INDEX_MAP_WRAPPER',
         0x11: 'ENCODED_BANK_MUX', 0x12: 'RAM_MUX', 0x13: 'ARCH_GROUP_STLMAP_BASE', 0x14: 'MUX',
         0x15: 'ARCH_GROUP_BASE', 0x16: 'MUX_STL_DATA', 0x17: 'MUX_DATA'}


def table(cls):
    if 'ARCH_GROUP' in cls:
        return 'arch'
    if cls.endswith('MUX'):
        return 'mux'
    return cls


# Members that hold pointers, and which table their back-references count in.
PTR_FIELDS = {
    'm_mux_path_list': 'mux', 'aux_mux': 'mux', 'next_mux_selection': 'mux', 'm_map_mux': 'mux',
    'm_polarity_mux': 'mux', 'm_labeled_simple_mux_list': 'mux', 'm_block_info': 'BLOCK_INFO',
    'm_group_index_map': 'INDEX_MAP_WRAPPER', 'm_offset_index_map': 'INDEX_MAP_WRAPPER',
    'm_arch_group': 'arch', 'm_arch_group_list': 'arch',
}

LINE = re.compile(r'^(\S+(?: \S+)?)\s+(?:(enum|bool|ptr)\s+|([+-]\d+)\s+)?(\t*)(.*)$')


def parse(path):
    lines = open(path, encoding='latin1').read().split('\n')
    # Skip to the body: first line `FFFFFFFF ptr`.
    start = next(i for i, l in enumerate(lines) if l.startswith('FFFFFFFF') and l.rstrip().endswith('ptr'))
    bodies = []          # (class, [(member, depth, raw)])
    cur = None
    last = {}            # table -> last id
    nxt = {}             # table -> next new id
    refs = []            # every pointer: (body index, member, table, id)
    for l in lines[start + 1:]:
        m = re.match(r'^ASMDB_(\w+?)\s*ptr\s*$', l)
        if m:
            cur = (m.group(1), [])
            bodies.append(cur)
            continue
        if cur is None or not l.strip() or '(BASE CLASS)' in l:
            continue
        val = (l[:24].split() or [''])[0]
        rest = l[24:]
        depth = len(rest) - len(rest.lstrip('\t'))
        member = rest.strip()
        base = re.sub(r'\[\d+\]', '', member)
        cur[1].append((member, depth, val))
        if base in PTR_FIELDS and re.fullmatch(r'[0-9A-F]{8}', val):
            # Container lengths share the member name at a shallower depth;
            # only elements (indexed, or scalar members) are pointers.
            if base in ('m_mux_path_list', 'm_arch_group_list', 'm_labeled_simple_mux_list') and '[' not in member:
                continue
            if base == 'm_mux_path_list' and member.count('[') == 1 and depth == 1:
                continue  # inner list length
            v = int(val, 16)
            if v >= 0x80000000:
                v -= 1 << 32
            if v == 0:
                ident = None
                t = PTR_FIELDS[base]
            elif v < 0:
                t = table(CLASS[-v])
                nxt[t] = nxt.get(t, 0) + 1
                ident = nxt[t]
                last[t] = ident
            else:
                t = PTR_FIELDS[base]
                ident = (last.get(t, 0) + v) & 0x3fffffff
                last[t] = ident
            refs.append((len(bodies) - 1, member, t, ident))
    # Assign ids to bodies per table in order.
    count = {}
    ids = []
    for cls, _ in bodies:
        t = table(cls)
        count[t] = count.get(t, 0) + 1
        ids.append((t, count[t]))
    return bodies, ids, refs, nxt


if __name__ == '__main__':
    bodies, ids, refs, nxt = parse(sys.argv[1])
    import collections
    print('bodies per table', collections.Counter(t for t, _ in ids))
    print('new ids per table', nxt)
    pickle.dump((bodies, ids, refs), open(sys.argv[2], 'wb'))
