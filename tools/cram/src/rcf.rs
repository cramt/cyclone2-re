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

/// `dest = ( cell, PORT ), route_port = PHYS;`: the net reaches `cell`'s
/// logical input `port`, physically entering on `route_port` (the fitter
/// permutes LUT inputs). It always follows the resource that feeds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dest {
    pub cell: String,
    pub port: String,
    pub route_port: Option<String>,
    /// Index into `Signal::resources` of the resource feeding this sink.
    pub from: Option<usize>,
}

impl Dest {
    pub fn physical_port(&self) -> &str {
        self.route_port.as_deref().unwrap_or(&self.port)
    }
}

#[derive(Debug, Clone)]
pub struct Signal {
    pub name: String,
    pub resources: Vec<Resource>,
    pub dests: Vec<Dest>,
}

fn parse_dest(s: &str, from: Option<usize>) -> Option<Dest> {
    let inner = s.split_once('(')?.1;
    let (cell_port, rest) = inner.split_once(')')?;
    let (cell, port) = cell_port.split_once(',')?;
    let route_port = rest.split("route_port = ").nth(1).map(|p| p.trim().to_string());
    Some(Dest { cell: cell.trim().to_string(), port: port.trim().to_string(), route_port, from })
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
    // Index of the resource the next dest hangs off: the last resource, or
    // the labelled one a `branch_point` jumps back to.
    let mut at: Option<usize> = None;
    let mut labels: Vec<(String, usize)> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("signal_name = ") {
            let name = rest.trim_end_matches('{').trim().to_string();
            cur = Some(Signal { name, resources: vec![], dests: vec![] });
            at = None;
            labels.clear();
        } else if l == "}" {
            if let Some(s) = cur.take() {
                sigs.push(s);
            }
        } else if let Some(s) = cur.as_mut() {
            let body = l.trim_end_matches(';');
            let bad = || format!("line {}: unrecognised rcf statement {l:?}", n + 1);
            if body.starts_with("dest") {
                let d = parse_dest(body, at).with_context(|| format!("line {}: bad dest {l:?}", n + 1))?;
                s.dests.push(d);
            } else if let Some(rest) = body.strip_prefix("label = ") {
                // `label = Label_KIND:XxYySzIi, KIND:XxYySzIi`: a resource
                // that later `branch_point`s can name.
                let (label, res) = rest.split_once(", ").with_context(bad)?;
                s.resources.push(parse_resource(res).with_context(bad)?);
                at = Some(s.resources.len() - 1);
                labels.push((label.to_string(), s.resources.len() - 1));
            } else if let Some(label) = body.strip_prefix("branch_point = ") {
                at = Some(labels.iter().rev().find(|(l, _)| l == label).with_context(bad)?.1);
            } else if let Some(r) = parse_resource(body) {
                s.resources.push(r);
                at = Some(s.resources.len() - 1);
            } else if !body.is_empty() {
                bail!(bad());
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
        assert_eq!(
            s[0].dests,
            vec![Dest { cell: "lut".into(), port: "DATAD".into(), route_port: Some("DATAB".into()), from: Some(1) }]
        );
    }
}
