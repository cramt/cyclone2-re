"""Post-fit Verilog netlist (quartus_eda --simulation, build/simulation/
modelsim/top.vo): every atom with its `// Location:` comment, defparams and
port connections. LUT masks here are already in physical input order (the
.vo agrees with the rcf's route_port)."""
import re

ATOM = re.compile(r'// Location: (\S+)\n(cycloneii_\w+) (\\\S+ |[^\s(]+) ?\((.*?)\);', re.S)
PORT = re.compile(r'\.(\w+)\((.*?)\)(?:,|\s*$)', re.S)
DEFP = re.compile(r'defparam (\\\S+ |\S+)\s*\.(\w+) = (.*?);')


def norm(n):
    n = n.strip()
    return n[1:].strip() if n.startswith('\\') else n


def parse(path):
    text = open(path).read()
    atoms = {}
    for loc, kind, name, ports in ATOM.findall(text):
        body = '\n'.join(l for l in ports.split('\n') if not l.strip().startswith('//'))
        p = {k: norm(v) for k, v in PORT.findall(body)}
        atoms[norm(name)] = {'loc': loc, 'kind': kind[len('cycloneii_'):], 'ports': p, 'params': {}}
    for name, k, v in DEFP.findall(text):
        name = norm(name)
        if name in atoms:
            atoms[name]['params'][k] = v.strip().strip('"')
    return atoms


def le_loc(loc):
    """LCCOMB_X36_Y2_N22 / LCFF_X36_Y2_N23 -> (x, y, le)."""
    m = re.match(r'LC(?:COMB|FF)_X(\d+)_Y(\d+)_N(\d+)', loc)
    x, y, n = map(int, m.groups())
    return x, y, n // 2


def lut_mask(a):
    m = a['params']['lut_mask']
    return int(m.split("'h")[1], 16)
