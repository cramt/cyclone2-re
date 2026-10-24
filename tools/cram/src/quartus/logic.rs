//! LEs, LABs and clock controls of a Quartus post-fit netlist (`vo`) and
//! its routing (`rcf`): LUT masks, carry and register wiring, which LAB
//! line feeds which LE input and LAB control, and which global clock buffer
//! feeds a LAB. chipsmith's `bitstream::logic` turns them into bits.

use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, HashMap};

use chipsmith_cyclone2::bitstream::logic::{
    clkctrl_select_code, clock_slots, Aclr, ClkCtrl, Ctl, Domain, GlobalClock, LabConfig, LeConfig, LeSource, Logic,
    Lut, Reg, Sload,
};

use super::m4k::instance_path;
use crate::rcf::Signal;
use crate::vo::{self, Atom};

fn port_index(p: &str) -> Option<usize> {
    Some(match p {
        "DATAA" => 0,
        "DATAB" => 1,
        "DATAC" => 2,
        "DATAD" => 3,
        // A register's sdata enters through its LE's datac input mux.
        "SDATA" => 2,
        _ => return None,
    })
}

/// Names a cell or its output goes by: the rcf uses the node name (`q66`,
/// `cnt[8]~39`, or the carry-out net `co1` for a LUT whose only output
/// is its carry), the .vo the instance (`f67`) and output nets
/// (`cnt[8]~39_combout`).
fn names(a: &Atom) -> Vec<String> {
    let mut v = vec![a.name.clone()];
    for p in ["regout", "combout", "cout"] {
        if let Some(n) = a.port(p).filter(|n| !n.is_empty()) {
            v.push(n.to_string());
            if let Some(s) = n.strip_suffix("_combout") {
                v.push(s.to_string());
            }
        }
    }
    v
}

/// From the Quartus post-fit netlist and rcf. Also returns what the
/// design uses that this model doesn't cover yet; those bits will be
/// missing from the output.
pub fn from_atoms(atoms: &[Atom], sigs: &[Signal]) -> Result<(Logic, Vec<String>)> {
    let mut out = Logic::default();
    let mut unmodelled = Vec::new();
    // LE slots: comb and ff atoms by (x, y, le).
    let mut slots: BTreeMap<(u32, u32, u32), (Option<&Atom>, Option<&Atom>)> = BTreeMap::new();
    let mut cell: HashMap<String, (u32, u32, u32)> = HashMap::new();
    for a in atoms {
        let Some(loc) = vo::le_loc(&a.loc) else { continue };
        let s = slots.entry(loc).or_default();
        match a.kind.as_str() {
            "lcell_comb" => s.0 = Some(a),
            "lcell_ff" => s.1 = Some(a),
            k => bail!("{}: unexpected {k} at an LE location", a.name),
        }
        for n in names(a) {
            cell.insert(n, loc);
        }
    }

    // Routing facts per LE / LAB, from the rcf.
    let mut inputs: HashMap<(u32, u32, u32), [Option<LeSource>; 4]> = HashMap::new();
    // LEs whose LUT (not register sdata) takes a routed datac.
    let mut lut_c_routed: std::collections::HashSet<(u32, u32, u32)> = Default::default();
    let mut ctl: HashMap<(u32, u32, u32), HashMap<String, u8>> = HashMap::new(); // port -> pia
    let mut glob: HashMap<(u32, u32, u32), HashMap<String, u8>> = HashMap::new(); // port -> buf
    let mut outputs: HashMap<(u32, u32, u32), [Option<String>; 3]> = HashMap::new();
    let mut labs: BTreeMap<(u32, u32), LabConfig> = BTreeMap::new();
    for s in sigs {
        for (i, r) in s.resources.iter().enumerate() {
            let (x, y) = (r.x as u32, r.y as u32);
            match r.kind.as_str() {
                "LE_BUFFER" => {
                    let o = outputs.entry((x, y, r.index as u32 / 2)).or_default();
                    o[r.index as usize % 2] = Some(instance_path(&s.name));
                }
                "LOCAL_LINE" => {
                    outputs.entry((x, y, r.index as u32)).or_default()[2] = Some(instance_path(&s.name));
                }
                "LAB_CONTROL_MUX" => {
                    let prev = s.parents[i].map(|j| &s.resources[j]);
                    let src = match prev.filter(|p| (p.x, p.y) == (r.x, r.y)) {
                        Some(p) if p.kind == "LOCAL_INTERCONNECT" => LeSource::Line(p.index as u8),
                        Some(p) if p.kind == "LOCAL_LINE" => LeSource::Feedback(p.index as u8),
                        _ => bail!("{}: LAB_CONTROL_MUX {r:?} not fed from its own LAB", s.name),
                    };
                    labs.entry((x, y)).or_default().pia[r.index as usize] = Some(src);
                }
                _ => {}
            }
        }
        for d in &s.dests {
            let Some(&loc) = cell.get(&instance_path(&d.cell)) else { continue };
            let Some(r) = d.from.map(|i| &s.resources[i]) else { continue };
            match r.kind.as_str() {
                "LOCAL_INTERCONNECT" | "LOCAL_LINE" => {
                    let Some(p) = port_index(d.physical_port()) else { continue };
                    let src = if r.kind == "LOCAL_LINE" {
                        LeSource::Feedback(r.index as u8)
                    } else {
                        LeSource::Line(r.index as u8)
                    };
                    inputs.entry(loc).or_default()[p] = Some(src);
                    if p == 2 && d.physical_port() != "SDATA" {
                        lut_c_routed.insert(loc);
                    }
                }
                "LAB_CONTROL_MUX" => {
                    ctl.entry(loc).or_default().insert(d.port.clone(), r.index as u8);
                }
                "BLK_CLK_BUF" => {
                    glob.entry(loc).or_default().insert(d.port.clone(), r.index as u8);
                }
                _ => {}
            }
        }
    }

    // LAB clocks: each register's domain, then `clock_slots`.
    let domain = |x: u32, y: u32, le: u32, ff: &Atom| -> Option<Domain> {
        let buf = *glob.get(&(x, y, le))?.get("CLK")?;
        let inv = ff.port("clk").is_some_and(|c| c.starts_with('!'));
        let ena = if ff.net("ena").is_some() {
            ctl.get(&(x, y, le)).and_then(|k| k.get("ENA")).map(|&line| Ctl { line, inverted: inverted(ff, "ena") })
        } else {
            None
        };
        Some((buf, inv, ena))
    };
    let mut lab_domains: BTreeMap<(u32, u32), Vec<Domain>> = BTreeMap::new();
    for (&(x, y, le), &(_, ff)) in &slots {
        let Some(ff) = ff else { continue };
        let Some(d) = domain(x, y, le, ff) else {
            unmodelled.push(format!("{}: clock not from a global buffer", ff.name));
            continue;
        };
        if ff.net("ena").is_some() && d.2.is_none() {
            unmodelled.push(format!("{}: clock enable not on a LAB control line", ff.name));
        }
        let v = lab_domains.entry((x, y)).or_default();
        if !v.contains(&d) {
            v.push(d);
        }
    }
    let mut domain_index: HashMap<((u32, u32), Domain), u8> = HashMap::new();
    for (&xy, v) in &lab_domains {
        let slot = clock_slots(v).with_context(|| format!("LAB {xy:?}"))?;
        let lab = labs.entry(xy).or_default();
        for (j, d) in slot.iter().enumerate() {
            if let Some(d) = d {
                lab.clocks[j] = Some(GlobalClock { buf: d.0, inverted: d.1 });
                lab.enables[j] = d.2;
                domain_index.insert((xy, *d), j as u8);
            }
        }
    }

    for (&(x, y, le), &(comb, ff)) in &slots {
        let lab = labs.entry((x, y)).or_default();
        let mut c = LeConfig {
            x,
            y,
            le,
            lut: None,
            inputs: inputs.get(&(x, y, le)).copied().unwrap_or_default(),
            carry_in: false,
            reg_feedback: false,
            reg: None,
            comb_out: [false; 3],
        };
        if let Some(comb) = comb {
            c.lut = Some(Lut { mask: comb.lut_mask().with_context(|| format!("{}: no lut_mask", comb.name))? });
            c.carry_in = comb.params.get("sum_lutc_input").map(String::as_str) == Some("cin") && comb.net("cin").is_some();
            // Own register on datac without a routed input: the
            // register feedback path (counter LSB: `cnt[0]~75`).
            let own = ff.and_then(|f| f.port("regout"));
            // The routed datac may carry the register's sdata instead
            // (packed register with sload high, M4K design d16).
            c.reg_feedback = own.is_some() && comb.port("datac") == own && !lut_c_routed.contains(&(x, y, le));
        }
        if let Some(ff) = ff {
            let g = glob.get(&(x, y, le));
            let k = ctl.get(&(x, y, le));
            let clock = domain(x, y, le, ff).and_then(|d| domain_index.get(&((x, y), d))).copied().unwrap_or(0);
            if inverted(ff, "aclr") {
                unmodelled.push(format!("{}: inverted aclr", ff.name));
            }
            let aclr = if ff.net("aclr").is_some() {
                let a = match (k.and_then(|k| k.get("ACLR")), g.and_then(|g| g.get("ACLR"))) {
                    (Some(&p), _) => Aclr::Pia(p),
                    (None, Some(&b)) => Aclr::Global(b),
                    _ => bail!("{}: aclr route not found", ff.name),
                };
                set_once(&mut lab.aclr, a, "aclr", (x, y))?;
                Some(0)
            } else {
                None
            };
            let mut sync = false;
            if ff.net("sclr").is_some() {
                let line = *k.and_then(|k| k.get("SCLR")).with_context(|| format!("{}: sclr route", ff.name))?;
                set_once(&mut lab.sync.sclr, Ctl { line, inverted: inverted(ff, "sclr") }, "sync clear", (x, y))?;
                sync = true;
            }
            match ff.port("sload") {
                Some("vcc") => {
                    set_once(&mut lab.sync.sload, Sload::Const, "sync load", (x, y))?;
                    sync = true;
                }
                Some(n) if n != "gnd" => {
                    let line = *k.and_then(|k| k.get("SLOAD")).with_context(|| format!("{}: sload route", ff.name))?;
                    let c = Ctl { line, inverted: inverted(ff, "sload") };
                    set_once(&mut lab.sync.sload, Sload::Pia(c), "sync load", (x, y))?;
                    sync = true;
                }
                _ => {}
            }
            c.reg = Some(Reg { clock, aclr, sync });
        }
        let outs = outputs.get(&(x, y, le)).cloned().unwrap_or_default();
        let comb_names = comb.map(names).unwrap_or_default();
        for (o, net) in outs.iter().enumerate() {
            c.comb_out[o] = net.as_ref().is_some_and(|n| comb_names.contains(n));
        }
        out.les.push(c);
    }
    out.labs = labs;
    // Clock control: the rcf runs pin -> CLKBUF_IN (z, input) on one
    // net and CLKBUF_OUT (z) -> GLOBAL_CLK_H (index = G) on the next;
    // match them up by tile and z.
    let mut ins: HashMap<(i16, i16, i16), u8> = HashMap::new();
    let mut outs: HashMap<(i16, i16, i16), u8> = HashMap::new();
    let mut cores: HashMap<(i16, i16, i16), u8> = HashMap::new();
    for s in sigs {
        for (i, r) in s.resources.iter().enumerate() {
            match r.kind.as_str() {
                "CLKBUF_IN" => {
                    ins.insert((r.x, r.y, r.z), r.index as u8);
                    let up = |j: Option<usize>| j.map(|j| (j, &s.resources[j]));
                    if let Some((j, _)) = up(s.parents[i]).filter(|(_, p)| p.kind == "BLOCK_INPUT_MUX") {
                        if let Some((_, l)) = up(s.parents[j]).filter(|(_, p)| p.kind == "LOCAL_INTERCONNECT") {
                            cores.insert((r.x, r.y, r.z), l.index as u8);
                        }
                    }
                }
                "GLOBAL_CLK_H" => {
                    if let Some(p) = s.parents[i].map(|p| &s.resources[p]).filter(|p| p.kind == "CLKBUF_OUT") {
                        outs.insert((p.x, p.y, p.z), r.index as u8);
                    }
                }
                _ => {}
            }
        }
    }
    for (k, &g) in &outs {
        let input = *ins.get(k).with_context(|| format!("clock control at {k:?} has no CLKBUF_IN"))?;
        if clkctrl_select_code(input).is_none() {
            unmodelled.push(format!("clock control G{g}: input {input} (select code unknown)"));
        }
        out.clkctrls.push(ClkCtrl { g, input, core_line: cores.get(k).copied() });
    }
    Ok((out, unmodelled))
}

fn inverted(a: &Atom, port: &str) -> bool {
    a.port(port).is_some_and(|n| n.starts_with('!'))
}

fn set_once<T: PartialEq + std::fmt::Debug>(slot: &mut Option<T>, v: T, what: &str, lab: (u32, u32)) -> Result<()> {
    match slot {
        Some(old) if *old != v => bail!("LAB {lab:?}: two {what}s ({old:?}, {v:?}), only one modelled"),
        _ => *slot = Some(v),
    }
    Ok(())
}
