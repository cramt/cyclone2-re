//! M4K RAM blocks of a Quartus post-fit netlist (`ram_block` atoms) and
//! their routing; chipsmith's `bitstream::m4k` turns them into bits.

use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeMap, HashMap};

use chipsmith_cyclone2::bitstream::m4k::*;

use crate::rcf::Signal;
use crate::vo::Atom;

fn param<'a>(a: &'a Atom, k: &str) -> Result<&'a str> {
    a.params.get(k).map(String::as_str).with_context(|| format!("{}: no {k}", a.name))
}

fn num(a: &Atom, k: &str) -> Result<u32> {
    param(a, k)?.parse().with_context(|| format!("{}: {k}", a.name))
}

fn sel(s: &str, what: &str) -> Result<Option<Sel>> {
    Ok(match s {
        "none" => None,
        "clock0" | "clear0" => Some(Sel::Zero),
        "clock1" | "clear1" => Some(Sel::One),
        _ => bail!("{what} = {s}"),
    })
}

fn read_port(a: &Atom, port: char) -> Result<ReadPort> {
    let w = num(a, &format!("port_{port}_address_width"))?;
    let width = Width::from_addr_bits(w).with_context(|| format!("{}: port {port} address width {w}", a.name))?;
    let clock = sel(param(a, &format!("port_{port}_data_out_clock"))?, "data_out_clock")?;
    let clear = sel(param(a, &format!("port_{port}_data_out_clear"))?, "data_out_clear")?;
    let out_reg = match (clock, clear) {
        (Some(clock), clear) => Some(OutReg { clock, clear }),
        (None, None) => None,
        (None, Some(_)) => bail!("{}: port {port} output clear without an output register", a.name),
    };
    Ok(ReadPort { width, out_reg })
}

/// `mem_init1 . mem_init0` as bits, LSB first.
fn mem_init(a: &Atom) -> Result<Vec<bool>> {
    let mut out = Vec::new();
    for k in ["mem_init0", "mem_init1"] {
        let Some(v) = a.params.get(k) else { continue };
        let (n, hex) = v.split_once("'h").with_context(|| format!("{}: {k} not hex", a.name))?;
        let n: usize = n.trim().parse()?;
        let mut bits = Vec::with_capacity(n);
        for ch in hex.trim().chars().rev() {
            let d = ch.to_digit(16).with_context(|| format!("{}: {k}", a.name))?;
            for i in 0..4 {
                bits.push(d >> i & 1 == 1);
            }
        }
        bits.resize(n, false);
        out.extend(bits);
    }
    Ok(out)
}

/// `section ram_sublocations` of the rcf: output bit -> its column slot.
/// Keys are (owner, port, bit). Quartus names an output bit one of two
/// ways: a per-bit slice `<ram>|q_b[5]` (owner `<ram>|`, the bit number of
/// the whole logical RAM), or, for RAMs whose outputs go through a depth
/// mux, the atom itself: `<atom>` (bit 0 of its first output port, port
/// `*` here) and `<atom>~PORTBDATAOUT3`. Owners are hierarchy paths with
/// the `entity:` parts dropped, which is how the post-fit netlist names the
/// atoms. Values are (half, slot): half is 0 except in a packed block,
/// where the atom on half 1 is port B.
pub type Sublocs = HashMap<(String, char, u32), (u32, u32)>;

/// `altsyncram:ram|altsyncram_t581:auto_generated|q_a[0]` ->
/// `ram|auto_generated|q_a[0]`.
pub fn instance_path(cell: &str) -> String {
    cell.split('|').map(|seg| seg.rsplit_once(':').map_or(seg, |(_, inst)| inst)).collect::<Vec<_>>().join("|")
}

/// A RAM output (or sink) cell name -> (owner, port, bit).
fn cell_key(cell: &str) -> (String, char, u32) {
    let p = instance_path(cell);
    if let Some((head, idx)) = p.strip_suffix(']').and_then(|s| s.rsplit_once('[')) {
        if let (Some(port), Ok(bit)) = (head.chars().last(), idx.parse()) {
            if let Some(owner) = head.strip_suffix(port).and_then(|h| h.strip_suffix("q_")) {
                return (owner.to_string(), port, bit);
            }
        }
    }
    for (port, tag) in [('a', "~PORTADATAOUT"), ('b', "~PORTBDATAOUT")] {
        if let Some((atom, bit)) = p.rsplit_once(tag) {
            if let Ok(bit) = bit.parse() {
                return (atom.to_string(), port, bit);
            }
        }
    }
    (p, '*', 0)
}

pub fn sublocations(rcf: &str) -> Result<Sublocs> {
    let mut out = HashMap::new();
    let Some(start) = rcf.find("section ram_sublocations") else { return Ok(out) };
    for l in rcf[start..].lines().skip(1) {
        let l = l.trim();
        if l == "}" {
            break;
        }
        // ( altsyncram:ram|...|q_b[5], 0, 4);
        let inner = l.trim_start_matches('(').trim_end_matches(';').trim_end_matches(')');
        let parts: Vec<&str> = inner.rsplitn(3, ',').map(str::trim).collect();
        ensure!(parts.len() == 3, "ram_sublocations line {l:?}");
        let (slot, field, cell) = (parts[0].parse::<u32>()?, parts[1].parse::<u32>()?, parts[2]);
        out.insert(cell_key(cell), (field, slot));
    }
    Ok(out)
}

/// The M4K a resource sits in, if any.
fn m4k_tile(x: i16, y: i16) -> Option<(u32, u32)> {
    COLUMNS.contains(&(x as u32)).then_some((x as u32, y as u32))
}

/// The RAM atoms of a post-fit netlist and their rcf routing -> blocks.
/// Also returns what isn't modelled (the block's bits are then missing).
pub fn from_atoms(atoms: &[Atom], sigs: &[Signal], rcf_text: &str) -> Result<(Vec<M4k>, Vec<String>)> {
    let mut unmodelled = Vec::new();
    let mut by_loc: BTreeMap<(u32, u32), Vec<&Atom>> = BTreeMap::new();
    for a in atoms.iter().filter(|a| a.kind == "ram_block") {
        let loc = a.loc.strip_prefix("M4K_X").and_then(|r| r.split_once("_Y")).and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)));
        by_loc.entry(loc.with_context(|| format!("{}: location {}", a.name, a.loc))?).or_default().push(a);
    }
    if by_loc.is_empty() {
        return Ok((vec![], unmodelled));
    }
    let subs = sublocations(rcf_text)?;
    let mut out = Vec::new();
    for (&(x, y), v) in &by_loc {
        let m = match v[..] {
            [a] => block(a, x, y, sigs, &subs, None).with_context(|| a.name.clone()),
            [p, q] => packed(p, q, x, y, sigs, &subs),
            _ => Err(anyhow::anyhow!("{} RAM atoms in one block", v.len())),
        };
        match m {
            Ok(m) => out.push(m),
            Err(e) => unmodelled.push(format!("M4K X{x} Y{y}: {e:#}")),
        }
    }
    Ok((out, unmodelled))
}

/// One atom's share of a block. `half` is set for one of the two
/// single-port atoms of a packed block (`packed`): the block is then 256 x 18
/// whatever the atom says, half 0 is port A on words 128-255, half 1 port B
/// on words 0-127 (fuzz s32, s36).
fn block(a: &Atom, x: u32, y: u32, sigs: &[Signal], subs: &Sublocs, half: Option<u32>) -> Result<M4k> {
    let op = param(a, "operation_mode")?;
    let mut pa = read_port(a, 'a')?;
    if half.is_some() {
        ensure!(op == "single_port", "packed {op} atom not modelled");
        pa.width = Width::X18;
    }
    let (mode, cell_port) = match op {
        // A single port reads x36 only through port B (fuzz: r36 sets port
        // B's write mode and read clock, its address enters on the read
        // address pins).
        "rom" if pa.width == Width::X36 => (Mode::SimpleDual { write: None, read: pa, read_clock: Sel::Zero }, 'a'),
        "single_port" | "rom" => (Mode::Single { a: pa }, 'a'),
        "dual_port" => {
            let read = read_port(a, 'b')?;
            let read_clock = sel(param(a, "port_b_address_clock")?, "port_b_address_clock")?.context("port B address unclocked")?;
            (Mode::SimpleDual { write: Some(pa.width), read, read_clock }, 'b')
        }
        "bidir_dual_port" => {
            let b = read_port(a, 'b')?;
            let b_clock = sel(param(a, "port_b_address_clock")?, "port_b_address_clock")?.context("port B address unclocked")?;
            (Mode::true_dual(pa, b, b_clock)?, 'a')
        }
        m => bail!("operation_mode {m} not modelled"),
    };
    // A RAM split over blocks: each atom holds its own slice of the words
    // (mem_init is per atom) and of the bits (its first bit number offsets
    // the rcf's per-bit slice names).
    let first = |p: char| -> u32 { a.params.get(&format!("port_{p}_first_bit_number")).and_then(|v| v.parse().ok()).unwrap_or(0) };
    let prefix = match a.name.rfind('|') {
        Some(i) => a.name[..=i].to_string(),
        None => String::new(),
    };
    // An atom's bits need not be contiguous slices of the RAM: a x36 RAM
    // split into two x18 atoms gives one of them bits 0-8 and 27-35 (s36,
    // w13), whatever its first bit number says. Atom bit k is the k-th of
    // its slices: in a packed block the sublocations on its half, otherwise
    // the output nets leaving this block's M4K_BUFFERs, if every bit has one.
    let mut cells: HashMap<char, Vec<u32>> = HashMap::new();
    if let Some(h) = half {
        let v = subs.iter().filter(|((o, p, _), (hh, _))| *o == prefix && *p == 'a' && *hh == h).map(|((_, _, b), _)| *b);
        cells.insert('a', v.collect());
    } else {
        for s in sigs {
            let Some(r) = s.resources.first() else { continue };
            if r.kind == "M4K_BUFFER" && m4k_tile(r.x, r.y) == Some((x, y)) {
                let (owner, p, b) = cell_key(&s.name);
                if owner == prefix {
                    cells.entry(p).or_default().push(b);
                }
            }
        }
        cells.retain(|p, v| a.params.get(&format!("port_{p}_data_width")).and_then(|w| w.parse::<usize>().ok()) == Some(v.len()));
    }
    for v in cells.values_mut() {
        v.sort();
        v.dedup();
    }
    // Slice name (bit number of the logical RAM) of atom bit `bit`, and back.
    let slice_of = |p: char, bit: u32| -> u32 {
        match cells.get(&p) {
            Some(c) => c.get(bit as usize).copied().unwrap_or(u32::MAX),
            None => bit + first(p),
        }
    };
    let atom_bit = |p: char, slice: u32| -> Option<u32> {
        match cells.get(&p) {
            Some(c) => c.iter().position(|&b| b == slice).map(|i| i as u32),
            None => slice.checked_sub(first(p)),
        }
    };
    let widths = |p: char| -> Result<Width> {
        Ok(match (mode, p) {
            (Mode::Single { a }, 'a') => a.width,
            (Mode::SimpleDual { write: Some(w), .. }, 'a') => w,
            (Mode::SimpleDual { write: None, read, .. }, 'a') => read.width,
            (Mode::SimpleDual { read, .. }, 'b') => read.width,
            (Mode::TrueDual { a, .. }, 'a') => a.width,
            (Mode::TrueDual { b, .. }, 'b') => b.width,
            _ => bail!("port {p} unused in {mode:?}"),
        })
    };
    let cell_width = widths(cell_port)?;
    // A sublocation slot is a column of the narrower port's word; bit k of
    // a wider port's slice sits in that port's (k / narrow width)-th narrow
    // word (fuzz: x8 write / x32 read lists slots 0-7 four times).
    let ports: &[char] = if matches!(mode, Mode::Single { .. }) { &['a'] } else { &['a', 'b'] };
    let mut narrow_port = 'a';
    for &p in ports {
        if widths(p)? < widths(narrow_port)? {
            narrow_port = p;
        }
    }
    let narrow = widths(narrow_port)?;
    let narrow_bits = num(a, &format!("port_{narrow_port}_data_width"))? as usize;
    // quartus_map keeps an atom from a VQM whole, named after its first
    // output net; its own atoms sit inside the altsyncram's hierarchy.
    let vqm_atom = half.is_none() && !a.name.contains('|');
    // Linear position, within one word of the widest port, of bit `bit` of
    // a slice, `idx` narrow words further on (a wide port's data pins on a
    // narrow slice are `PORTADATAIN[idx]`).
    let column = |port: char, bit: u32, idx: u32| -> Result<usize> {
        let key = |owner: &str, p: char, b: u32| subs.get(&(owner.to_string(), p, b));
        // An atom read from a VQM (`cram quartus-export`) isn't split into
        // per-bit slices: the rcf lists its output nets, each at (0, 0), and
        // bit k sits in column k of the narrow word (VQM experiment q1:
        // output k on M4K_BUFFER k, contents in that order).
        let (_, s) = key(&prefix, port, slice_of(port, bit))
            .or_else(|| key(&a.name, port, bit))
            .or_else(|| if bit == 0 { key(&a.name, '*', 0) } else { None })
            .or_else(|| key(&prefix, cell_port, slice_of(cell_port, bit)))
            .copied()
            .or_else(|| vqm_atom.then_some((0, bit % narrow_bits as u32)))
            .with_context(|| format!("no ram_sublocation for port {port} bit {bit}"))?;
        // A packed half's slots count both halves' 36 columns.
        let s = if half.is_some() { s as usize % 18 } else { s as usize };
        Ok(narrow.linear(bit as usize / narrow_bits + idx as usize, s))
    };

    // Contents: mem_init is in the words of the port whose outputs the rcf
    // names (port A for single port, B for simple dual); fit on every fuzz
    // design.
    let mut content = Content::default();
    let w = num(a, &format!("port_{cell_port}_data_width"))? as usize;
    let word0 = if half == Some(0) { 128 } else { 0 };
    for (n, &b) in mem_init(a)?.iter().enumerate() {
        if b {
            content.set_linear(cell_width.linear(word0 + n / w, column(cell_port, (n % w) as u32, 0)?));
        }
    }

    // Inputs and controls from the rcf: every sink whose last resource is in
    // this block's tile.
    let mut m = M4k { x, y, mode, controls: Controls::default(), inputs: BTreeMap::new(), content };
    for s in sigs {
        for d in &s.dests {
            let Some(r) = d.from.map(|i| &s.resources[i]) else { continue };
            if m4k_tile(r.x, r.y) != Some((x, y)) {
                continue;
            }
            let (port, idx) = match d.port.split_once('[') {
                Some((p, i)) => (p, i.trim_end_matches(']').parse::<u32>()?),
                None => (d.port.as_str(), 0),
            };
            // Which data bit a data/byte-enable sink is: on a per-bit slice
            // cell, the slice's bit (and `idx` narrow words on); on an atom
            // cell, bit `idx` of the port.
            let (owner, cport, cbit) = cell_key(&d.cell);
            let port_idx = idx;
            let (cbit, idx) = if owner == a.name || vqm_atom {
                (idx, 0)
            } else {
                match atom_bit(cport, cbit) {
                    Some(b) => (b, idx),
                    // The other atom of a packed block.
                    None if half.is_some() => continue,
                    None => bail!("slice {cbit} below the atom's first bit"),
                }
            };
            let line = || -> Result<u8> {
                ensure!(r.kind == "LOCAL_INTERCONNECT", "{} {}: fed by {}", d.cell, d.port, r.kind);
                Ok(r.index as u8)
            };
            let ctl = || -> Result<(CtlSrc, Option<(usize, u8)>)> {
                Ok(match r.kind.as_str() {
                    "BLK_CLK_BUF" => (CtlSrc::Global(r.index as u8), None),
                    k if k.ends_with("CONTROL_INPUT") => {
                        let prev = s.parents[d.from.unwrap()].map(|p| &s.resources[p]);
                        let prev = prev.filter(|p| p.kind == "LOCAL_INTERCONNECT").with_context(|| format!("{k} not fed by a LAB line"))?;
                        let k = r.index as usize;
                        ensure!(k < 8, "control input {k}");
                        (CtlSrc::Control(k as u8), Some((k, prev.index as u8)))
                    }
                    k => bail!("{} {}: control from {k}", d.cell, d.port),
                })
            };
            match port {
                "PORTAADDR" | "PORTBADDR" => {
                    let b = port == "PORTBADDR" || matches!(mode, Mode::SimpleDual { write: None, .. });
                    let w = widths(if b && half.is_none() { 'b' } else { 'a' })?;
                    let inp = Input::addr(b || half == Some(1), port_idx + 12 - w.addr_bits())?;
                    put_input(&mut m.inputs, inp, line()?)?;
                }
                "PORTADATAIN" | "PORTBDATAIN" => {
                    // Bit idx of the slice named by the cell: the idx-th
                    // word of the cell's port inside one word of this port.
                    let b = port == "PORTBDATAIN";
                    let w = widths(if b { 'b' } else { 'a' })?;
                    ensure!(cport == cell_port || owner == a.name || vqm_atom, "data sink on cell of port {cport}");
                    let col = w.column(column(if b { 'b' } else { 'a' }, cbit, idx)?);
                    let inp = Input::Data((col + if b || half == Some(1) { 18 } else { 0 }) as u8);
                    put_input(&mut m.inputs, inp, line()?)?;
                }
                "PORTABYTEENAMASKS" => {
                    // On an atom cell the index is the byte, not a bit.
                    let byte = if owner == a.name || vqm_atom { port_idx as usize } else { widths('a')?.column(column('a', cbit, idx)?) / 9 };
                    let inp = Input::ByteEna(byte as u8);
                    put_input(&mut m.inputs, inp, line()?)?;
                }
                "PORTBREWE" if !matches!(mode, Mode::TrueDual { .. }) => bail!("port B read enable not modelled"),
                "PORTAWE" | "PORTBREWE" | "CLK0" | "CLK1" | "ENA0" | "ENA1" | "CLR0" | "CLR1" => {
                    let (src, pia) = ctl()?;
                    if let Some((k, l)) = pia {
                        put(&mut m.controls.pia[k], l, "control input")?;
                    }
                    let c = &mut m.controls;
                    let (slot, what) = match port {
                        "PORTAWE" if half == Some(1) => (&mut c.b_write_enable, "port B write enable"),
                        "PORTAWE" => (&mut c.write_enable, "write enable"),
                        "PORTBREWE" => (&mut c.b_write_enable, "port B write enable"),
                        "CLK0" => (&mut c.clock[0], "clock"),
                        "CLK1" => (&mut c.clock[1], "clock"),
                        "ENA0" => (&mut c.enable[0], "clock enable"),
                        "ENA1" => (&mut c.enable[1], "clock enable"),
                        "CLR0" => (&mut c.clear[0], "clear"),
                        _ => (&mut c.clear[1], "clear"),
                    };
                    put(slot, src, what)?;
                }
                p => bail!("RAM port {p} not modelled"),
            }
        }
    }
    for p in ["portawe", "portbrewe", "clk0", "clk1", "clr0", "clr1"] {
        ensure!(!a.port(p).is_some_and(|n| n.starts_with('!')), "inverted {p} not modelled");
    }
    for k in 0..2 {
        m.controls.enable_inverted[k] = m.controls.enable[k].is_some() && a.port(&format!("ena{k}")).is_some_and(|n| n.starts_with('!'));
    }
    for p in ["portaaddrstall", "portbaddrstall"] {
        ensure!(a.net(p).is_none(), "{p} not modelled");
    }
    Ok(m)
}

/// Two single-port atoms sharing one block (packed mode). Which is port A
/// is the rcf's sublocation half of their output bits.
fn packed(p: &Atom, q: &Atom, x: u32, y: u32, sigs: &[Signal], subs: &Sublocs) -> Result<M4k> {
    let half_of = |a: &Atom| -> Result<u32> {
        let prefix = a.name.rfind('|').map_or("", |i| &a.name[..=i]);
        let first: u32 = a.params.get("port_a_first_bit_number").and_then(|v| v.parse().ok()).unwrap_or(0);
        let (h, _) = subs.get(&(prefix.to_string(), 'a', first)).with_context(|| format!("{}: no sublocation", a.name))?;
        Ok(*h)
    };
    let (a0, a1) = match (half_of(p)?, half_of(q)?) {
        (0, 1) => (p, q),
        (1, 0) => (q, p),
        h => bail!("packed atoms on halves {h:?}"),
    };
    let m0 = block(a0, x, y, sigs, subs, Some(0)).with_context(|| a0.name.clone())?;
    let m1 = block(a1, x, y, sigs, subs, Some(1)).with_context(|| a1.name.clone())?;
    let out = |m: &M4k| match m.mode {
        Mode::Single { a } => Ok(a.out_reg),
        _ => bail!("packed half is not single port"),
    };
    let mut m = M4k { mode: Mode::Packed { a_out: out(&m0)?, b_out: out(&m1)? }, ..m0.clone() };
    for (&inp, &line) in &m1.inputs {
        put_input(&mut m.inputs, inp, line)?;
    }
    let (c, d) = (&mut m.controls, &m1.controls);
    for k in 0..8 {
        if let Some(l) = d.pia[k] {
            put(&mut c.pia[k], l, "control input")?;
        }
    }
    for k in 0..2 {
        for (slot, v) in [(&mut c.clock[k], d.clock[k]), (&mut c.enable[k], d.enable[k]), (&mut c.clear[k], d.clear[k])] {
            if let Some(v) = v {
                put(slot, v, "shared control")?;
            }
        }
        c.enable_inverted[k] |= d.enable_inverted[k];
    }
    if let Some(v) = d.b_write_enable {
        put(&mut c.b_write_enable, v, "port B write enable")?;
    }
    for l in (0..BITS).filter(|&l| m1.content.get_linear(l)) {
        m.content.set_linear(l);
    }
    Ok(m)
}

fn put<T: PartialEq + std::fmt::Debug + Copy>(slot: &mut Option<T>, v: T, what: &str) -> Result<()> {
    match slot {
        Some(old) if *old != v => bail!("two different {what} sources ({old:?}, {v:?})"),
        _ => *slot = Some(v),
    }
    Ok(())
}

fn put_input(inputs: &mut BTreeMap<Input, u8>, inp: Input, line: u8) -> Result<()> {
    match inputs.insert(inp, line) {
        Some(old) if old != line => bail!("{inp:?} fed from lines {old} and {line}"),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sublocations() {
        let s = sublocations("x\nsection ram_sublocations {\n ( altsyncram:ram|altsyncram_tbg1:auto_generated|q_b[2], 0, 1);\n}\n").unwrap();
        assert_eq!(s.get(&("ram|auto_generated|".to_string(), 'b', 2)), Some(&(0, 1)));
        assert_eq!(cell_key("altsyncram:ram|altsyncram_8032:auto_generated|ram_block1a1~PORTBDATAOUT0"), ("ram|auto_generated|ram_block1a1".to_string(), 'b', 0));
    }
}
