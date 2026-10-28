//! Embedded multipliers of a Quartus post-fit netlist (`mac_mult` and
//! `mac_out` atoms) and the lines its rcf routes into them; chipsmith's
//! `bitstream::mult` turns them into bits.
//!
//! What the netlist says, observed (README "Embedded multipliers"): a
//! `mac_mult` sits at `DSPMULT_X39_Y<y>_N<row>`, its `mac_out` at
//! `DSPOUT_X39_Y<y>_N<row + 2>`. Operands come MSB-aligned: a narrow
//! operand's low bits are `gnd` in the post-fit atom. The rcf names the
//! multiplier after one of its product nets (`p0[0]`; with unused low
//! product bits, the first real one), which are the `mac_out`'s `dataa`;
//! data sinks are `DATAA[i]`/`DATAB[i]` fed by a
//! LAB line, signs `SIGNA`/`SIGNB` fed by a line through
//! `DSP_CONTROL_INPUT` 3/4. `CLK`, `ENA`, `ACLR` of either atom come from a
//! row clock (`BLK_CLK_BUF`) or a line through `DSP_CONTROL_INPUT` 0, 1, 2.

use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeMap;

use chipsmith_cyclone2::bitstream::mult::{Ctl, DataIn, DataPin, Line, Mult, Registers, Sign, X};

use crate::rcf::Signal;
use crate::vo::Atom;

/// `{a, b, c}` (MSB first) or a single net -> nets LSB first.
fn bus(s: &str) -> Vec<String> {
    let inner = s.trim().trim_start_matches('{').trim_end_matches('}');
    let mut v: Vec<String> = inner.split(',').map(|n| n.trim().trim_start_matches('\\').trim().to_string()).collect();
    v.reverse();
    v
}

fn param<'a>(a: &'a Atom, k: &str) -> Result<&'a str> {
    a.params.get(k).map(String::as_str).with_context(|| format!("{}: no {k}", a.name))
}

/// `DSPMULT_X39_Y5_N1` -> (5, 1).
fn loc(a: &Atom, prefix: &str) -> Result<(u32, u32)> {
    let r = a.loc.strip_prefix(prefix).with_context(|| format!("{}: location {}", a.name, a.loc))?;
    let (x, r) = r.split_once("_Y").context("location")?;
    let (y, n) = r.split_once("_N").context("location")?;
    ensure!(x.parse::<u32>()? == X, "{}: multiplier at X{x}", a.name);
    Ok((y.parse()?, n.parse()?))
}

/// The multiplier atoms of a post-fit netlist and their rcf routing ->
/// blocks. Also returns what isn't modelled (that block's bits are then
/// missing).
pub fn from_atoms(atoms: &[Atom], sigs: &[Signal]) -> Result<(Vec<Mult>, Vec<String>)> {
    let mut unmodelled = Vec::new();
    let mut by_y: BTreeMap<u32, Vec<(u32, &Atom)>> = BTreeMap::new();
    for a in atoms.iter().filter(|a| a.kind == "mac_mult") {
        let (y, row) = loc(a, "DSPMULT_X")?;
        by_y.entry(y).or_default().push((row, a));
    }
    let outs: Vec<&Atom> = atoms.iter().filter(|a| a.kind == "mac_out").collect();
    let mut blocks = Vec::new();
    for (y, rows) in by_y {
        match block(y, &rows, &outs, sigs) {
            Ok(m) => blocks.push(m),
            Err(e) => unmodelled.push(format!("multiplier X{X} Y{y}: {e:#}")),
        }
    }
    Ok((blocks, unmodelled))
}

fn block(y: u32, rows: &[(u32, &Atom)], outs: &[&Atom], sigs: &[Signal]) -> Result<Mult> {
    let width = |a: &Atom, k: &str| -> Result<usize> { Ok(param(a, k)?.parse()?) };
    // Nine-bit mode when every multiplier of the block fits a 9 x 9 row.
    let mut nine_bit = true;
    for (_, a) in rows {
        nine_bit &= width(a, "dataa_width")? <= 9 && width(a, "datab_width")? <= 9;
    }
    ensure!(nine_bit || rows.len() == 1, "two multipliers in an 18-bit block");
    let row_bits = if nine_bit { 9 } else { 18 };
    let mut m = Mult { y, nine_bit, data: BTreeMap::new(), sign: [Sign::Signed; 2], registers: None };
    let mut input = [[false; 2]; 2];
    let mut sign_reg = [false; 2];
    let mut output = [false; 2];
    // (cell name, atom) of every atom with a register, for its controls.
    let mut clocked: Vec<&Atom> = Vec::new();
    for &(row, a) in rows {
        ensure!(row < 2, "{}: row {row}", a.name);
        let out = outs
            .iter()
            .find(|o| loc(o, "DSPOUT_X").is_ok_and(|l| l == (y, row + 2)))
            .with_context(|| format!("{}: no mac_out beside it", a.name))?;
        let reg = |a: &Atom, p: &str| -> Result<bool> {
            Ok(match param(a, p)? {
                "none" => false,
                "0" => true,
                v => bail!("{}: {p} = {v}", a.name),
            })
        };
        // The rcf's name for this multiplier: one of its product nets.
        let products = bus(out.port("dataa").context("mac_out without dataa")?);
        let cell = sigs
            .iter()
            .flat_map(|s| &s.dests)
            .map(|d| &d.cell)
            .find(|c| products.contains(c))
            .with_context(|| format!("{}: the rcf routes nothing into it", a.name))?
            .clone();
        // An 18-bit operand spans both rows' registers.
        let rows_of: Vec<usize> = if nine_bit { vec![row as usize] } else { vec![0, 1] };
        let mut any = false;
        for (op, p) in [(0, "dataa_clock"), (1, "datab_clock")] {
            if reg(a, p)? {
                any = true;
                for &r in &rows_of {
                    input[r][op] = true;
                }
            }
        }
        for (op, p) in [(0, "signa_clock"), (1, "signb_clock")] {
            if reg(a, p)? {
                any = true;
                sign_reg[op] = true;
            }
        }
        if any {
            clocked.push(a);
        }
        if reg(out, "output_clock")? {
            for &r in &rows_of {
                output[r] = true;
            }
            clocked.push(out);
        }
        for (k, port, base) in [(0, "dataa", 0), (1, "datab", 18)] {
            let w = width(a, &format!("{port}_width"))?;
            let nets = bus(a.port(port).with_context(|| format!("{}: {port} unconnected", a.name))?);
            ensure!(nets.len() == w, "{}: {port} has {} nets, width {w}", a.name, nets.len());
            let _ = k;
            for (i, n) in nets.iter().enumerate() {
                let pin = DataPin::new((base + row as usize * 9 + row_bits - w + i) as u32)?;
                let d = match n.as_str() {
                    "vcc" => continue,
                    "gnd" => DataIn::Zero,
                    _ => DataIn::Line(line(sigs, &cell, &format!("{}[{i}]", port.to_uppercase()), y)?),
                };
                ensure!(m.data.insert(pin, d).is_none(), "{}: data input {pin:?} twice", a.name);
            }
        }
        for (k, port) in [(0, "signa"), (1, "signb")] {
            let s = match a.port(port) {
                None | Some("vcc") => Sign::Signed,
                Some("gnd") => bail!("{}: {port} tied to gnd", a.name),
                Some(_) => Sign::Line(line(sigs, &cell, &port.to_uppercase(), y)?),
            };
            // Both rows share the block's sign inputs.
            if rows.len() > 1 && m.sign[k] != Sign::Signed {
                ensure!(m.sign[k] == s, "{}: rows disagree on {port}", a.name);
            }
            m.sign[k] = s;
        }
    }
    if !clocked.is_empty() {
        // The block has one clock, enable and clear: every sink of one in
        // the block's tile is it.
        let mut has = (false, false);
        for a in &clocked {
            for p in ["clk", "aclr", "ena"] {
                ensure!(!a.port(p).is_some_and(|n| n.starts_with('!')), "{}: inverted {p} not modelled", a.name);
            }
            has.0 |= a.net("aclr").is_some();
            has.1 |= a.net("ena").is_some();
        }
        let clock = ctl(sigs, "CLK", y)?;
        let clear = if has.0 { Some(ctl(sigs, "ACLR", y)?.context("clear not routed")?) } else { None };
        let enable = match (has.1, ctl(sigs, "ENA", y)?) {
            (false, _) => None,
            (true, Some(Ctl::Line(l))) => Some(l),
            (true, c) => bail!("enable from {c:?}"),
        };
        m.registers = Some(Registers {
            clock: clock.context("clock not routed")?,
            enable,
            clear,
            input,
            sign: sign_reg,
            output,
        });
    }
    Ok(m)
}

/// What drives the block's control `port` (every sink of it in the tile
/// must agree): a row clock, or a line through a control input. `None` when
/// the rcf routes none.
fn ctl(sigs: &[Signal], port: &str, y: u32) -> Result<Option<Ctl>> {
    let mut found = None;
    for s in sigs {
        for d in s.dests.iter().filter(|d| d.port == port) {
            let i = d.from.context("sink without a resource")?;
            let r = &s.resources[i];
            if (r.x as u32, r.y as u32) != (X, y) {
                continue;
            }
            let c = match r.kind.as_str() {
                "BLK_CLK_BUF" => Ctl::global(r.index as u32)?,
                "DSP_CONTROL_INPUT" => {
                    let p = &s.resources[s.parents[i].context("control input without a parent")?];
                    ensure!(p.kind == "LOCAL_INTERCONNECT", "{port}: control input fed by {}", p.kind);
                    Ctl::Line(Line::new(p.index as u32)?)
                }
                k => bail!("{port}: fed by {k}"),
            };
            match found {
                Some(old) => ensure!(old == c, "two {port}s: {old:?} and {c:?}"),
                None => found = Some(c),
            }
        }
    }
    Ok(found)
}

/// The LAB line the rcf routes into `cell`'s `port`, directly (data) or
/// through a `DSP_CONTROL_INPUT` (signs).
fn line(sigs: &[Signal], cell: &str, port: &str, y: u32) -> Result<Line> {
    for s in sigs {
        for d in s.dests.iter().filter(|d| d.cell == cell && d.port == port) {
            let mut r = &s.resources[d.from.context("sink without a resource")?];
            if r.kind == "DSP_CONTROL_INPUT" {
                let p = s.parents[d.from.unwrap()].context("control input without a parent")?;
                r = &s.resources[p];
            }
            ensure!(r.kind == "LOCAL_INTERCONNECT", "{cell} {port}: fed by {}", r.kind);
            ensure!((r.x as u32, r.y as u32) == (X, y), "{cell} {port}: line at X{} Y{}", r.x, r.y);
            return Line::new(r.index as u32);
        }
    }
    bail!("{cell} {port}: not routed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buses_read_lsb_first() {
        assert_eq!(bus("{\\a~1 ,b,gnd}"), ["gnd", "b", "a~1"]);
        assert_eq!(bus("x"), ["x"]);
    }
}
