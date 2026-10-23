# For each distinct (cell arc, RF, delay) in quartus_sta reports, list the
# DMF leaves that explain it. Usage: dmf_sta_check.py dmf.tsv rows.tsv...
# (dmf.tsv from `ddb dmf <file> <names>`, rows from sta_rows.py). A value is
# explained if some leaf for that arc has it: which transition subnode and
# LUT-mask key Quartus picks is not modelled.
import sys,collections,re
dmf=[l.rstrip('\n').split('\t') for l in open(sys.argv[1])]
def val(v):
    try: return int(v)
    except: return None
def leaves(pathre, keyre):
    return [(p,k,val(v)) for p,k,v in dmf if re.fullmatch(pathre,p) and re.fullmatch(keyre,k) and val(v) is not None]
seen=collections.Counter()
for f in sys.argv[2:]:
    for l in open(f):
        t=l.rstrip('\n').split('\t')
        if t[0]=='CELL':
            # Carry variant from the LE index: 7 -> 8 crosses the LAB's
            # carry-select block, 15 leaves the LAB.
            le=int(t[6].rsplit('_N',1)[1])//2 if t[1]=='LCCOMB' else 0
            ch='CHAIN_ACROSS_LAB' if le==15 else 'CHAIN_THROUGH_LCB' if le==7 else 'CHAIN_BASE'
            seen[(t[1],t[2],t[3],t[4],t[5],ch if t[1]=='LCCOMB' else '')]+=1
        if t[0]=='MICRO': seen[('MICRO',t[1],'','',t[2],'')]+=1
ok=bad=0; nrows_ok=nrows=0
out=[]
for (lt,inp,outp,tr,v,loc),n in sorted(seen.items()):
    ps=round(float(v)*1000)
    expl=[]
    if lt=='LCCOMB':
        I='I:'+inp.upper(); O='O:'+outp.upper()
        chain=loc
        if I=='I:DATAC' and O=='O:COMBOUT':
            # register feedback into its own LUT's datac
            for p,k,x in leaves('LE_COMB/MICRO', 'REG_FEEDBACK_C_TO_COMBOUT'):
                if x==ps: expl.append(f'{p} [{k}]')
        if O=='O:COMBOUT':
            # any transition subnode, any used-input-mask key
            for p,k,x in leaves(f'LE_COMB(/RC_\\w+/RC_\\w+)?/P2P/InOut:P2P_IN_OUT', f'{I} {O}( \\S+)?'):
                if x==ps: expl.append(f'{p} [{k}]')
        else:
            for p,k,x in leaves('LE_COMB/P2P/InOut:P2P_IN_OUT', f'{I} {O} {chain}'):
                if x==ps: expl.append(f'{p} [{k}]')
                for p2,k2,y in leaves('LE_COMB/TABLE', f'INV_CARRY {chain}'):
                    if x+y==ps and I=='I:CIN': expl.append(f'{p} [{k}] + {p2} [{k2}]')
    elif lt=='LCFF':
        if inp in ('-',''):
            for p,k,x in leaves('LE_FF/P2P/BuriedOut:P2P_BUR_OUT', 'B:FF O:REGOUT'):
                if x==ps: expl.append(f'{p} [{k}]')
        for p,k,x in leaves('LE_FF/P2P/InBuried:P2P_IN_BUR', f'I:{inp.upper()} B:FF'):
            if x==ps: expl.append(f'{p} [{k}]')
    elif lt=='MICRO':
        name={'uTco':'TCO','uTsu':'TSU','uTh':'TH'}[inp]
        for p,k,x in leaves('LE_FF/FLIP-FLOP', f'B:FF {name}'):
            if abs(x)==ps: expl.append(f'{p} [{k}] ({x})')
    else:
        continue
    nrows+=n
    if expl: ok+=1; nrows_ok+=n
    else: bad+=1
    out.append(f"{'OK ' if expl else 'XX '} {lt} {inp}->{outp} {tr} {v} {loc} x{n}: {' | '.join(expl[:6])}")
print('\n'.join(out))
print(f'distinct (arc,RF,value): {ok} explained, {bad} not; rows {nrows_ok}/{nrows}')
