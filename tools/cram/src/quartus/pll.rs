//! The PLLs of a Quartus post-fit netlist (`cycloneii_pll` atoms, every
//! parameter the fitter settled) and the clock pin its rcf routes into
//! each; chipsmith's `bitstream::pll` turns them into bits.

use anyhow::{bail, Context, Result};

use chipsmith_cyclone2::bitstream::pll::{Config, Divider, Feedback, Loop, Output, Pll, Site};

use crate::rcf::Signal;
use crate::vo::Atom;

fn param<'a>(a: &'a Atom, k: &str) -> Result<&'a str> {
    Ok(a.params.get(k).with_context(|| format!("{}: no {k}", a.name))?.trim().trim_matches('"').trim())
}

fn num(a: &Atom, k: &str) -> Result<u32> {
    param(a, k)?.parse().with_context(|| format!("{}: {k}", a.name))
}

/// A parameter the netlist may leave at its atom default
/// (eda/sim_lib/cycloneii_atoms.v).
fn num_or(a: &Atom, k: &str, default: u32) -> Result<u32> {
    if a.params.contains_key(k) { num(a, k) } else { Ok(default) }
}

/// A counter's divider from its `<c>_high`, `_low`, `_mode`.
fn divider(a: &Atom, c: &str) -> Result<Option<Divider>> {
    let mode = param(a, &format!("{c}_mode"))?;
    let odd = match mode {
        "bypass" => return Ok(None),
        "even" => false,
        "odd" => true,
        m => bail!("{}: {c}_mode {m}", a.name),
    };
    Ok(Some(Divider::new(num(a, &format!("{c}_high"))? as u8, num(a, &format!("{c}_low"))? as u8, odd)?))
}

pub fn from_atoms(atoms: &[Atom], sigs: &[Signal]) -> Result<(Vec<Pll>, Vec<String>)> {
    let mut out = Vec::new();
    let mut unmodelled = Vec::new();
    for a in atoms.iter().filter(|a| a.kind == "pll") {
        let site = Site::from_name(&a.loc).with_context(|| format!("{}: location {}", a.name, a.loc))?;
        let feedback = match param(a, "operation_mode")? {
            "normal" => Feedback::Normal,
            "no_compensation" => Feedback::NoCompensation,
            m => bail!("{}: operation_mode {m}", a.name),
        };
        for (port, idle) in [("ena", "vcc"), ("clkswitch", "gnd"), ("testclearlock", "gnd"), ("sbdin", "gnd")] {
            if a.port(port).is_some_and(|n| n != idle) {
                unmodelled.push(format!("{}: {port} in use", a.name));
            }
        }
        // A counter is in use when an output port names it (`clk<k>_counter`);
        // altpll leaves the others out, a hand-made atom may name a bypassed
        // one.
        let used: Vec<String> = (0..6).filter_map(|k| a.params.get(&format!("clk{k}_counter")).map(|v| v.trim().trim_matches('"').to_string())).collect();
        let mut outputs = [None; 3];
        for (k, o) in outputs.iter_mut().enumerate() {
            let c = format!("c{k}");
            if !used.contains(&c) {
                continue;
            }
            *o = Some(Output {
                divider: divider(a, &c)?,
                initial: num_or(a, &format!("{c}_initial"), 1)? as u8,
                phase: num_or(a, &format!("{c}_ph"), 0)? as u8,
            });
        }
        // The atom gives m as a number; its high/low split is even (or
        // high = low + 1 for an odd m), as in every fuzz design.
        let m = num(a, "m")?;
        let m = Divider::new(m.div_ceil(2) as u8, (m / 2) as u8, m % 2 == 1)?;
        let r: f64 = param(a, "loop_filter_r")?.parse()?;
        let n = num(a, "n")? as u8;
        let config = Config {
            inclk_period_ps: num(a, "inclk0_input_frequency")?,
            n,
            m,
            m_initial: num(a, "m_initial")? as u8,
            m_phase: num(a, "m_ph")? as u8,
            vco_div2: a.params.get("vco_post_scale").is_some_and(|v| v.trim() == "2"),
            outputs,
            feedback,
            r#loop: Loop {
                charge_pump: num(a, "charge_pump_current")? as u16,
                filter_r_tenths: (r * 10.0).round() as u16,
                filter_c: num(a, "loop_filter_c")? as u16,
            },
        };
        // The clock pin: CLK_BUFFER (S = its index on the side) ->
        // CLKPIN_TO_PLLCLKIN_MUX -> PLL_CLKIN at the PLL.
        let (px, py) = site.xy();
        let mut pin = None;
        for s in sigs {
            for (i, r) in s.resources.iter().enumerate() {
                if r.kind == "PLL_CLKIN" && (r.x as u32, r.y as u32) == (px, py) {
                    let mut at = s.parents[i];
                    while let Some(j) = at {
                        if s.resources[j].kind == "CLK_BUFFER" {
                            pin = Some(s.resources[j].z as u8);
                        }
                        at = s.parents[j];
                    }
                }
            }
        }
        let inclk_pin = pin.with_context(|| format!("{}: no clock pin routed into {}", a.name, site.name()))?;
        // Control inputs: LOCAL_INTERCONNECT (the PLL tile's) -> PLL_*_PIN.
        let control = |kind: &str| -> Option<u8> {
            sigs.iter().find_map(|s| {
                s.resources.iter().enumerate().find_map(|(i, r)| {
                    (r.kind == kind && (r.x as u32, r.y as u32) == (px, py))
                        .then(|| s.parents[i].map(|j| &s.resources[j]))
                        .flatten()
                        .filter(|l| l.kind == "LOCAL_INTERCONNECT")
                        .map(|l| l.index as u8)
                })
            })
        };
        let areset = control("PLL_ARESET_PIN");
        let pfdena = control("PLL_PFD_ENA_PIN");
        for (port, idle, got) in [("areset", "gnd", areset), ("pfdena", "vcc", pfdena)] {
            if a.port(port).is_some_and(|n| n != idle) && got.is_none() {
                unmodelled.push(format!("{}: {port} not from a LAB line", a.name));
            }
        }
        out.push(Pll { site, config, inclk_pin, areset, pfdena });
    }
    Ok((out, unmodelled))
}
