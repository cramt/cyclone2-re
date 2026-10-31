//! Pins of a Quartus post-fit netlist (`cycloneii_io` atoms), its
//! back-annotated qsf and its routing; chipsmith's `bitstream::io` turns
//! them into bits.

use anyhow::{bail, ensure, Context, Result};
use std::collections::HashMap;

use chipsmith_cyclone2::bitstream::io::{Dir, InReg, Input, IoClock, IoControl, IoReg, OutData, Output, OutputEnable, PinUse, Standard, UnusedPins};

use crate::{rcf, vo};

/// A netlist's `I/O Standard` note.
fn standard(s: &str) -> Result<Standard> {
    Ok(match s {
        "3.3-V LVTTL" => Standard::Lvttl33,
        "3.3-V LVCMOS" => Standard::Lvcmos33,
        "2.5 V" => Standard::V25,
        "1.8 V" => Standard::V18,
        "1.5 V" => Standard::V15,
        _ => bail!("I/O standard {s:?} isn't modelled"),
    })
}

/// `set_instance_assignment -name <NAME> ON -to <port>` in a qsf.
fn qsf_on(qsf: &str, name: &str) -> Vec<String> {
    qsf.lines()
        .filter_map(|l| {
            let r = l.trim().strip_prefix("set_instance_assignment -name ")?.strip_prefix(name)?.trim_start();
            let r = r.strip_prefix("ON -to ")?;
            Some(r.trim().trim_matches('"').to_string())
        })
        .collect()
}

/// Pin usage from the post-fit netlist (`cycloneii_io` atoms: location,
/// mode, current strength), the back-annotated qsf (pull-up, bus hold)
/// and the rcf (which local lines feed the pin, which input path it uses).
pub fn pin_uses(atoms: &[vo::Atom], sigs: &[rcf::Signal], qsf: &str) -> Result<Vec<PinUse>> {
    let pull = qsf_on(qsf, "WEAK_PULL_UP_RESISTOR");
    let hold = qsf_on(qsf, "ENABLE_BUS_HOLD_CIRCUITRY");
    // rcf sinks into pins: (port, DATAIN|OE) -> local line feeding it.
    let mut lines: HashMap<(String, String), u32> = HashMap::new();
    let mut datain: HashMap<String, [bool; 2]> = HashMap::new();
    // Pins a PLL's external clock output drives (rcf IO_RAW_DATAOUT).
    let mut pll_clock: std::collections::HashSet<String> = Default::default();
    // (port, INCLK|OUTCLK) -> I/O clock buffer index.
    let mut clocks: HashMap<(String, String), u32> = HashMap::new();
    for s in sigs {
        for d in &s.dests {
            let Some(from) = d.from else { continue };
            if ["INCLK", "OUTCLK"].contains(&d.port.as_str()) {
                let r = &s.resources[from];
                ensure!(r.kind == "BLK_CLK_BUF", "{}: I/O register clock from {}", s.name, r.kind);
                clocks.insert((d.cell.clone(), d.port.clone()), r.index as u32);
                continue;
            }
            if s.resources[from].kind == "IO_RAW_DATAOUT" && d.port == "DATAIN" {
                pll_clock.insert(d.cell.clone());
                continue;
            }
            // Output data and enable, and the register controls (clock
            // enables, asynchronous reset, synchronous clear), all from a
            // local line through the cell's source muxes.
            if !["IO_DATAOUT", "IO_OE_PIN", "IO_CEIN_PIN", "IO_CEOUT_PIN", "IO_ACLR_PIN", "IO_SCLR_PIN"]
                .contains(&s.resources[from].kind.as_str())
            {
                continue;
            }
            let l = s.parents[from].map(|p| &s.resources[p]).filter(|r| r.kind == "LOCAL_INTERCONNECT");
            let l = l.with_context(|| format!("{}: pin sink not fed by a local line", s.name))?;
            lines.insert((d.cell.clone(), d.port.clone()), l.index as u32);
        }
        for r in s.resources.iter().filter(|r| r.kind == "IO_DATAIN") {
            ensure!((0..2).contains(&r.index), "IO_DATAIN index {}", r.index);
            datain.entry(s.name.clone()).or_default()[r.index as usize] = true;
        }
    }
    let mut out = Vec::new();
    for a in atoms.iter().filter(|a| a.kind == "io") {
        // The dedicated JTAG pins Quartus puts in the netlist when a design
        // instantiates the JTAG block: not user I/O, nothing to configure.
        if a.port("padio").is_some_and(|p| p.starts_with("altera_reserved_")) {
            continue;
        }
        let pin = a.loc.strip_prefix("PIN_").with_context(|| format!("{}: location {}", a.name, a.loc))?.to_string();
        let port = a.port("padio").with_context(|| format!("{}: no padio", a.name))?.to_string();
        let standard = standard(a.note("I/O Standard").with_context(|| format!("{}: no I/O standard", a.name))?)?;
        // A register control from the rcf (the local line into the cell's
        // mux for it) and the netlist (`!net`: inverted in the cell).
        let control = |p: &str, sink: &str| -> Result<Option<IoControl>> {
            let Some(n) = a.net(p) else { return Ok(None) };
            let line = *lines.get(&(port.clone(), sink.to_string())).with_context(|| format!("{}: {sink} not in the rcf", a.name))?;
            Ok(Some(IoControl { line, inverted: n.starts_with('!') }))
        };
        let areset = control("areset", "ARESET")?;
        let sclr = control("sreset", "SRESET")?;
        // `which`: input, output or oe; `clk`: the rcf sink of its clock,
        // `ena` its enable port.
        let reg = |which: &str, clk: &str, ena: &str| -> Result<Option<IoReg>> {
            let param = |k: &str| a.params.get(&format!("{which}_{k}")).map(String::as_str);
            if param("register_mode") != Some("register") {
                return Ok(None);
            }
            let buffer = *clocks.get(&(port.clone(), clk.to_string())).with_context(|| format!("{}: {clk} not in the rcf", a.name))?;
            let powers_up_high = param("power_up") == Some("high");
            let inverted = a.port(&clk.to_lowercase()).is_some_and(|c| c.starts_with('!'));
            // The output side's one enable line: a register not using it
            // is `tie_off_<which>_clock_enable` (held-out ioce h1, v1).
            let tied = a.params.get(&format!("tie_off_{which}_clock_enable")).is_some_and(|v| v == "true");
            let enable = control(&format!("{}ena", clk.to_lowercase()), ena)?.filter(|_| !tied);
            let areset = match param("async_reset") {
                None | Some("none") => None,
                // The cell's reset restores the power-up value.
                Some(m @ ("clear" | "preset")) => {
                    ensure!((m == "preset") == powers_up_high, "{}: {which} {m} with power-up {powers_up_high}", a.name);
                    Some(areset.with_context(|| format!("{}: {which} {m} without areset", a.name))?)
                }
                Some(m) => bail!("{}: {which}_async_reset {m:?}", a.name),
            };
            let sclr = match param("sync_reset") {
                None | Some("none") => None,
                Some("clear") => Some(sclr.with_context(|| format!("{}: {which} sync clear without sreset", a.name))?),
                Some(m) => bail!("{}: {which}_sync_reset {m:?} isn't modelled", a.name),
            };
            Ok(Some(IoReg { clock: IoClock { buffer, inverted, enable }, powers_up_high, areset, sclr }))
        };
        let input = || -> Result<Input> {
            // The rcf names the input's core net: the pin itself (or the
            // netlist's combout net) for the pad, the register output for
            // the register.
            let paths = |nets: &[Option<&str>]| {
                nets.iter().flatten().filter_map(|n| datain.get(*n)).fold([false; 2], |acc, d| [acc[0] | d[0], acc[1] | d[1]])
            };
            let comb = paths(&[Some(port.as_str()), a.net("combout")]);
            let reg = reg("input", "INCLK", "INCLKENA")?.map(|reg| InReg { reg, paths: paths(&[a.net("regout")]) });
            Ok(Input { comb, reg })
        };
        let output = || -> Result<Output> {
            let drive = match a.note("Current Strength") {
                None | Some("Default") => None,
                Some(s) => Some(s.strip_suffix("mA").and_then(|n| n.parse().ok()).with_context(|| format!("{}: current {s}", a.name))?),
            };
            // `!net`: the IOE inverts (quartus_map moves inverters feeding
            // pins, and an OE's, into it).
            let data_reg = reg("output", "OUTCLK", "OUTCLKENA")?;
            let data = match (a.port("datain"), lines.get(&(port.clone(), "DATAIN".into()))) {
                (Some(c @ ("gnd" | "vcc")), _) if data_reg.is_none() => OutData::Const(c == "vcc"),
                (_, None) if data_reg.is_none() && pll_clock.contains(&port) => OutData::PllClock,
                (d, Some(&line)) => OutData::Line { line, inverted: d.is_some_and(|d| d.starts_with('!')), reg: data_reg },
                (d, _) => bail!("{}: output data {d:?} neither routed nor constant (registered {})", a.name, data_reg.is_some()),
            };
            let open_drain = a.params.get("open_drain_output").is_some_and(|v| v == "true");
            let oe_reg = reg("oe", "OUTCLK", "OUTCLKENA")?;
            let enable = match (a.port("oe"), lines.get(&(port.clone(), "OE".into()))) {
                (Some("vcc"), _) if oe_reg.is_none() => if open_drain { OutputEnable::OpenDrain } else { OutputEnable::Always },
                (Some("gnd"), _) if oe_reg.is_none() && !open_drain => OutputEnable::Never,
                (oe, Some(&line)) if !open_drain => OutputEnable::Line { line, inverted: oe.is_some_and(|o| o.starts_with('!')), reg: oe_reg },
                (oe, _) => bail!("{}: output enable {oe:?} (open drain {open_drain}, registered {}) isn't modelled", a.name, oe_reg.is_some()),
            };
            let o = Output { drive, data, enable };
            o.clock().with_context(|| a.name.clone())?;
            Ok(o)
        };
        let dir = match a.params.get("operation_mode").map(String::as_str) {
            Some("input") => Dir::In(input()?),
            Some("output") => Dir::Out(output()?),
            Some("bidir") => Dir::Bidir(input()?, output()?),
            m => bail!("{}: operation_mode {m:?}", a.name),
        };
        out.push(PinUse { pin, standard, pull_up: pull.contains(&port), bus_hold: hold.contains(&port), dir, port });
    }
    Ok(out)
}

/// From a qsf; absent means our oracle's setting, input tri-stated.
pub fn unused_pins(qsf: &str) -> Result<UnusedPins> {
    let mut mode = UnusedPins::InputTriStated;
    for l in qsf.lines() {
        let Some(v) = l.trim().strip_prefix("set_global_assignment -name RESERVE_ALL_UNUSED_PINS ") else { continue };
        mode = match v.trim().trim_matches('"') {
            "AS INPUT TRI-STATED" => UnusedPins::InputTriStated,
            "AS INPUT TRI-STATED WITH BUS-HOLD" => UnusedPins::InputBusHold,
            "AS INPUT TRI-STATED WITH WEAK PULL-UP" => UnusedPins::InputPullUp,
            "AS OUTPUT DRIVING GROUND" | "AS OUTPUT DRIVING AN UNSPECIFIED SIGNAL" => UnusedPins::OutputGround,
            m => bail!("RESERVE_ALL_UNUSED_PINS {m:?} isn't modelled"),
        };
    }
    Ok(mode)
}
