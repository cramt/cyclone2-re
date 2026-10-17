"""Residual (non-routing) flipped bits of fuzz designs, in CRAM (frame, row).

    python3 tools/explore/residual.py <cram check verbose output is not needed> <design dir>...

Uses the Rust tool's mapping re-implemented here for speed of iteration.
"""
import re, sys, collections
sys.path.insert(0, 'tools/explore')

def flat_rowcol(frame, bit):
    byte, lane = divmod(bit, 8)
    pos = 311 - byte
    row = 2176 + pos if lane == 7 else lane * 311 + pos
    return row, frame

def ylane(row):
    y = 35 - (row - 18) // 70
    return y, (row - 18) % 70

def observed(path):
    o = {}
    for line in open(path):
        p = line.split()
        if len(p) == 3 and p[1].startswith('f') and 'crc' not in p[1]:
            f, b = p[1][1:].split(':')
            o[(int(f), int(b))] = int(p[2][-1])
    return o

def placements(d):
    pl = []
    for line in open(d + '/build/top.qsf'):
        m = re.search(r'LCCOMB_X(\d+)_Y(\d+)_N(\d+) -to (\S+)', line)
        if m:
            pl.append(tuple(map(int, m.groups()[:3])) + (m.group(4),))
    return pl

def le_rows(y, n):
    """CRAM rows of LE n//2 of LAB row y: LEs 0-7 count up from the LAB's
    first row, LEs 8-15 count down from its last, with 6 LAB-wide rows
    (32..37) between them."""
    l = n // 2
    base = 18 + 70 * (35 - y)
    off = 4 * l if l < 8 else 38 + 4 * (15 - l)
    return list(range(base + off, base + off + 4))
