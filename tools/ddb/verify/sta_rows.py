# Turn quartus_sta `report_timing -detail full_path -show_routing` reports
# into one row per cell arc / routing element / micro parameter (TSV), for
# verify/dmf_sta_check.py and RE delay statistics.
# Parse quartus_sta full_path reports (-show_routing) into rows:
#   CELL <loctype> <in_port> <out_port> <RF> <incr>
#   RE <re_name> <location> <incr> <fanout>
#   MICRO <type> <incr>
import sys,re,collections
rows=[]
for f in sys.argv[1:]:
    prev_ic=None
    for l in open(f,encoding='latin1'):
        if not l.startswith(';'): continue
        c=[x.strip() for x in l.strip().strip(';').split(';')]
        if len(c)!=7: continue
        total,incr,rf,typ,fan,loc,elem=c
        try: incr=float(incr)
        except: continue
        if typ=='IC':
            prev_ic=(loc,elem.split('|')[-1] if '|' in elem else elem)
        elif typ=='CELL':
            lt=re.sub(r'_X\d+_Y\d+_N\d+$','',loc)
            outp=elem.split('|')[-1] if '|' in elem else elem
            inp=prev_ic[1] if prev_ic and prev_ic[0]==loc else '-'
            rows.append(('CELL',lt,inp,outp,rf,incr,loc))
            prev_ic=None
        elif typ=='RE':
            rows.append(('RE',elem,loc,incr,fan))
        elif typ in('uTco','uTsu','uTh'):
            rows.append(('MICRO',typ,incr,loc))
for r in rows: print('\t'.join(map(str,r)))
