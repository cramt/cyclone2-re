import csv, pickle, os, collections
P='specimens/db/magellan4/'
def load():
    pk='/tmp/m4.pkl'
    if os.path.exists(pk): return pickle.load(open(pk,'rb'))
    nodes={}; byloc=collections.defaultdict(list)
    for r in csv.DictReader(open(P+'nodes.csv')):
        n=int(r['node']); t=(int(r['x']),int(r['y']),int(r['z']),int(r['index']),int(r['element_enum']),int(r['length']),int(r['direction']),r['name'])
        nodes[n]=t; byloc[t[:4]].append(n)
    out=collections.defaultdict(dict)  # src -> dst -> (edge, bits)
    for r in csv.DictReader(open(P+'edges.csv')):
        out[int(r['src'])][int(r['dst'])]=(int(r['src_edge']), tuple(r['asm_bits'].split()) if r['asm_bits'] else ())
    d=(nodes,dict(byloc),dict(out)); pickle.dump(d,open(pk,'wb')); return d
