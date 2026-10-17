import sys
# Rebuild the same canonical form from a PDB_ASCII_DUMP of the routing ddb.
def sv(h,bits):
    v=int(h,16)
    return v-(1<<bits) if v>=1<<(bits-1) else v
out=sys.stdout
el=None; tm=None; ids={'next':1,'last':0}
def ptr(h):
    v=sv(h,32)
    if v<0: i=ids['next']; ids['next']+=1
    else: i=(ids['last']+v)&0x3fffffff
    ids['last']=i; return i
cur=None; mode=None; offs=[]; pos=[]
def flush_t():
    if cur is not None:
        out.write('T '+' '.join(str(x) for x in cur)+''.join(' '+str(v) for v in offs)+' |'+''.join(' '+str(v) for v in pos)+'\n')
for ln in open(sys.argv[1],'rb'):
    ln=ln.decode('latin1').rstrip('\n'); f=ln.split()
    if not f: continue
    name=f[-1]
    if name=='m_x': ex=sv(f[0],16)
    elif name=='m_y': ey=sv(f[0],16)
    elif name=='m_fanout_edge_base': eb=f[0]
    elif name=='m_template': out.write(f'E {ex} {ey} {eb} {ptr(f[0])}\n')
    elif ln.startswith('DYGR_ROUTE_ELEMENT_TEMPLATEptr'):
        flush_t(); cur=[]; offs=[]; pos=[]; mode='t'
    elif mode=='t':
        if name in ('m_element_enum','m_num_edges','m_z','m_index') : cur.append(sv(f[0],16))
        elif name=='m_first_purely_redundant_edge' and len(f)==3: cur.append(sv(f[0],16))
        elif name in ('m_length','m_metal_layer','m_direction') and len(f)==3 and not f[0].startswith("'"): cur.append(sv(f[0],8))
        elif name.startswith('m_fanout_list_offsets['): offs.append(sv(f[0],32))
        elif name.startswith('m_fanout_to_positions['): pos.append(sv(f[0],16))
flush_t()
