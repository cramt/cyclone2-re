import csv, sys
# Check `ddb pins` CSV against Quartus fit reports:
#   python3 pins_fit_check.py pins.csv top.fit.rpt...
# "All Package Pins": every ball's pad number, bank and I/O type.
# "Input/Output/Bidir Pins": X, Y and cell number of every used pin.
pins = {}
for r in csv.DictReader(open(sys.argv[1])):
    pins.setdefault(r['pin'], []).append(r)

def tables(path):
    cur, header = None, None
    for ln in open(path, errors='replace'):
        if not ln.startswith(';'):
            if ln.startswith('+') and cur and header is None:
                continue
            if not ln.strip():
                cur = None
            continue
        cells = [c.strip() for c in ln.strip().strip(';').split(';')]
        if len(cells) == 1:
            cur, header = cells[0], None
        elif cur and header is None:
            header = cells
        elif cur:
            yield cur, dict(zip(header, cells))

stats = {'package pins': 0, 'pad mismatch': 0, 'bank mismatch': 0, 'type mismatch': 0, 'clock as row/col': 0,
         'used pins': 0, 'xyz mismatch': 0, 'unknown pin': 0}
bad = []
for path in sys.argv[2:]:
    for table, row in tables(path):
        if table == 'All Package Pins':
            stats['package pins'] += 1
            ours = pins.get(row['Location'])
            if not ours:
                stats['unknown pin'] += 1; bad.append((path, row['Location'], 'unknown')); continue
            o = ours[0]
            if o['pad'] != row['Pad Number']:
                stats['pad mismatch'] += 1; bad.append((path, row['Location'], 'pad', o['pad'], row['Pad Number']))
            if o['pad'] and o['bank'] != row['I/O Bank']:
                stats['bank mismatch'] += 1; bad.append((path, row['Location'], 'bank', o['bank'], row['I/O Bank']))
            # The report files the 16 dedicated clock inputs under the edge
            # they sit on.
            if o['io_type'] == 'Dedicated Clock' and row['I/O Type'] in ('Row I/O', 'Column I/O'):
                stats['clock as row/col'] += 1
            elif row['I/O Type'] not in ('--', '') and o['io_type'] != row['I/O Type']:
                stats['type mismatch'] += 1; bad.append((path, row['Location'], 'type', o['io_type'], row['I/O Type']))
        elif table in ('Input Pins', 'Output Pins', 'Bidir Pins') and 'X coordinate' in row:
            # (failed fits list pins as "Unassigned" without coordinates)
            stats['used pins'] += 1
            ours = pins.get(row['Pin #'])
            want = (row['X coordinate'], row['Y coordinate'], row['Cell number'])
            if not ours or (ours[0]['x'], ours[0]['y'], ours[0]['z']) != want:
                stats['xyz mismatch'] += 1; bad.append((path, row['Pin #'], 'xyz', ours and (ours[0]['x'], ours[0]['y'], ours[0]['z']), want))
print(len(sys.argv) - 2, 'reports', stats)
for b in bad[:20]:
    print(*b)
sys.exit(1 if bad else 0)
