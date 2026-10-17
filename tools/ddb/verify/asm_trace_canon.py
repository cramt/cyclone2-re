import sys
# Canonical per-node form of DYGR_ROUTE_ASM nodes from a PDB_ASCII_DUMP.
out=sys.stdout; node=None; starts=[]; bits=[]; cur=None
def sv(h,b):
    v=int(h,16); return v-(1<<b) if v>=1<<(b-1) else v
def flush():
    if node is not None:
        out.write(f"N {node[0]} {node[1]} |"+''.join(' '+str(s) for s in starts)+' |'+''.join(' '+','.join(map(str,b)) for b in bits)+'\n')
for ln in open(sys.argv[1],'rb'):
    f=ln.split()
    if not f: continue
    name=f[-1]
    if name.startswith(b'm_route_asm_nodes['):
        flush(); node=[None,None]; starts=[]; bits=[]
    elif name==b'm_num_bit_groups': node[0]=int(f[0],16)
    elif name==b'm_num_bits': node[1]=int(f[0],16)
    elif name.startswith(b'm_start_bit_index['): starts.append(int(f[0],16))
    elif name.startswith(b'm_asm_bits['): bits.append([]); 
    elif len(f)==3 and name in (b'm_flat_address',b'm_use_encoded_setting',b'm_is_encoded_bit_high',b'm_is_strangely_encoded',b'm_is_cff_bit') and not f[0].startswith(b"'"):
        bits[-1].append(int(f[0],16))
flush()
