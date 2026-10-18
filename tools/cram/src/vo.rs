//! Post-fit Verilog netlist (`quartus_eda --simulation --tool=modelsim`,
//! build/simulation/modelsim/top.vo). Every atom carries a `// Location:`
//! comment, and LUT masks and port connections are in *physical* terms
//! (they agree with the rcf's `route_port`), which makes it the one place
//! a placed design's logic can be read without re-deriving the fitter's
//! input permutation.

use anyhow::{Context, Result};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Atom {
    pub name: String,
    /// e.g. `lcell_comb`, `lcell_ff`, `clkctrl`, `io`.
    pub kind: String,
    /// `LCCOMB_X36_Y2_N22`, `CLKCTRL_G2`, `PIN_N2`, ...
    pub loc: String,
    /// Port -> connected net, `vcc`/`gnd` for constants; a leading `!`
    /// marks an inverted connection.
    pub ports: HashMap<String, String>,
    pub params: HashMap<String, String>,
}

fn norm(n: &str) -> String {
    let n = n.trim();
    n.strip_prefix('\\').map(|s| s.trim().to_string()).unwrap_or_else(|| n.to_string())
}

/// `.port(net),` pairs of an instance body, comments stripped.
fn ports(body: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let clean: String = body.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    let mut rest = clean.as_str();
    while let Some(i) = rest.find('.') {
        rest = &rest[i + 1..];
        let Some(open) = rest.find('(') else { break };
        let port = rest[..open].trim().to_string();
        // Nets may contain parentheses only inside escaped names, which
        // end at whitespace; find the matching close paren by depth.
        let mut depth = 0;
        let mut end = None;
        for (k, ch) in rest[open..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + k);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        let net = rest[open + 1..end].trim();
        let net = match net.strip_prefix('!') {
            Some(n) => format!("!{}", norm(n)),
            None => norm(net),
        };
        out.insert(port, net);
        rest = &rest[end + 1..];
    }
    out
}

pub fn parse(text: &str) -> Vec<Atom> {
    let mut atoms = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(loc) = line.trim().strip_prefix("// Location: ") {
            let loc = loc.split([',', ' ', '\t']).next().unwrap_or("").to_string();
            let Some(head) = lines.next() else { break };
            let Some(rest) = head.trim().strip_prefix("cycloneii_") else { continue };
            let (kind, rest) = rest.split_once(' ').unwrap_or((rest, ""));
            let name = rest.trim().trim_end_matches('(').trim();
            let mut body = String::new();
            for l in lines.by_ref() {
                body.push_str(l);
                body.push('\n');
                if l.trim_end().ends_with(");") {
                    break;
                }
            }
            by_name.insert(norm(name), atoms.len());
            atoms.push(Atom { name: norm(name), kind: kind.to_string(), loc, ports: ports(&body), params: HashMap::new() });
        } else if let Some(rest) = line.trim().strip_prefix("defparam ") {
            // defparam \name .param = value;
            let Some((name, rest)) = rest.split_once(" .").or_else(|| rest.split_once('.')) else { continue };
            let Some((param, value)) = rest.split_once('=') else { continue };
            if let Some(&i) = by_name.get(&norm(name)) {
                let v = value.trim().trim_end_matches(';').trim().trim_matches('"').to_string();
                atoms[i].params.insert(param.trim().to_string(), v);
            }
        }
    }
    atoms
}

pub fn read(path: &std::path::Path) -> Result<Vec<Atom>> {
    Ok(parse(&std::fs::read_to_string(path).with_context(|| path.display().to_string())?))
}

/// `LCCOMB_X36_Y2_N22` / `LCFF_X36_Y2_N23` -> (x, y, LE 0..15).
pub fn le_loc(loc: &str) -> Option<(u32, u32, u32)> {
    let rest = loc.strip_prefix("LCCOMB_X").or_else(|| loc.strip_prefix("LCFF_X"))?;
    let (x, rest) = rest.split_once("_Y")?;
    let (y, n) = rest.split_once("_N")?;
    Some((x.parse().ok()?, y.parse().ok()?, n.parse::<u32>().ok()? / 2))
}

impl Atom {
    pub fn port(&self, p: &str) -> Option<&str> {
        self.ports.get(p).map(String::as_str)
    }
    /// A port tied to a real net (not vcc/gnd/unconnected).
    pub fn net(&self, p: &str) -> Option<&str> {
        self.port(p).filter(|n| !matches!(*n, "vcc" | "gnd" | ""))
    }
    pub fn lut_mask(&self) -> Option<u16> {
        let m = self.params.get("lut_mask")?;
        u16::from_str_radix(m.split("'h").nth(1)?, 16).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_atoms() {
        let t = "// Location: LCCOMB_X36_Y2_N22\ncycloneii_lcell_comb \\cnt[8]~39 (\n// Equation(s):\n// x\n\n\t.dataa(vcc),\n\t.datab(cnt[8]),\n\t.cin(\\cnt[7]~38 ),\n\t.combout(\\cnt[8]~39_combout ),\n\t.cout(\\cnt[8]~40 ));\n// synopsys translate_off\ndefparam \\cnt[8]~39 .lut_mask = 16'h3C3F;\ndefparam \\cnt[8]~39 .sum_lutc_input = \"cin\";\n\n// Location: LCFF_X6_Y5_N3\ncycloneii_lcell_ff f67(\n\t.clk(!\\CLOCK_50~clkctrl_outclk ),\n\t.regout(q66));\n";
        let a = parse(t);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].name, "cnt[8]~39");
        assert_eq!(a[0].port("cin"), Some("cnt[7]~38"));
        assert_eq!(a[0].lut_mask(), Some(0x3c3f));
        assert_eq!(a[0].params["sum_lutc_input"], "cin");
        assert_eq!(a[1].name, "f67");
        assert_eq!(a[1].port("clk"), Some("!CLOCK_50~clkctrl_outclk"));
        assert_eq!(le_loc("LCFF_X6_Y5_N3"), Some((6, 5, 1)));
    }
}
