//! Quartus routing constraints file (`quartus_cdb --back_annotate=routing`).
//!
//! Each `signal_name = <net> { ... }` lists routing resources such as
//! `R4:X58Y19S0I16;` in drive order, then `dest = ( <cell>, <port> )`.
//! We only need the resources; which earlier resource drives each one is
//! recovered from the routing graph, so the tree syntax doesn't matter.

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub kind: String,
    pub x: i16,
    pub y: i16,
    /// `S`: the routing element's z.
    pub z: i16,
    /// `I`: the routing element's index.
    pub index: i16,
}

#[derive(Debug, Clone)]
pub struct Signal {
    pub name: String,
    pub resources: Vec<Resource>,
    /// Raw `dest = ...` lines; the LE/IO input mux they imply is block
    /// config (asmdb), not routing.
    pub dests: Vec<String>,
}

fn parse_resource(s: &str) -> Option<Resource> {
    let (kind, rest) = s.split_once(':')?;
    let rest = rest.strip_prefix('X')?;
    let (x, rest) = rest.split_once('Y')?;
    let (y, rest) = rest.split_once('S')?;
    let (z, i) = rest.split_once('I')?;
    Some(Resource {
        kind: kind.to_string(),
        x: x.parse().ok()?,
        y: y.parse().ok()?,
        z: z.parse().ok()?,
        index: i.parse().ok()?,
    })
}

pub fn parse(text: &str) -> Result<Vec<Signal>> {
    let mut sigs: Vec<Signal> = Vec::new();
    let mut cur: Option<Signal> = None;
    for (n, line) in text.lines().enumerate() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("signal_name = ") {
            let name = rest.trim_end_matches('{').trim().to_string();
            cur = Some(Signal { name, resources: vec![], dests: vec![] });
        } else if l == "}" {
            if let Some(s) = cur.take() {
                sigs.push(s);
            }
        } else if let Some(s) = cur.as_mut() {
            let body = l.trim_end_matches(';');
            if body.starts_with("dest") {
                s.dests.push(body.to_string());
            } else if let Some(r) = parse_resource(body) {
                s.resources.push(r);
            } else if !body.is_empty() && !body.starts_with("branch_point") {
                bail!("line {}: unrecognised rcf statement {l:?}", n + 1);
            }
        }
    }
    Ok(sigs)
}

pub fn read(path: &std::path::Path) -> Result<Vec<Signal>> {
    parse(&std::fs::read_to_string(path).with_context(|| path.display().to_string())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_readme_example() {
        let s = parse(
            "signal_name = SW[3] {\n\tCLK_BUFFER:X33Y0S0I0;\n\tR4:X58Y19S0I16;\n\
             \tdest = ( lut, DATAD ), route_port = DATAB;\n}\n",
        )
        .unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].resources[1], Resource { kind: "R4".into(), x: 58, y: 19, z: 0, index: 16 });
        assert_eq!(s[0].dests.len(), 1);
    }
}
