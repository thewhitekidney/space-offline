//! Node-graph static data, loaded from the content bundle's `nodegraphs/`.
//!
//! The files are written by the `nodegraphs` content section
//! (`tools/content_nodegraphs.py`) from the client's `nodegraphs`, `nodetypes`
//! and `atoms` tables (decoded by `tools/fsd.py`, edges by
//! `CFSD_ELEMENT_KINDS`). Layout, one row per line, tab-separated, `#` lines
//! are headers; every field escapes `% ; = | , TAB LF CR` as `%XX`:
//!
//! - `graphs.tsv`: `graph_id name tags parent`
//! - `nodes.tsv`: `graph_id node type atom params atom_params edges`
//! - `nodetypes.tsv`: `type_id name in_ports out_ports defaults outputs`
//! - `atoms.tsv`: `atom_id name atom_type defaults outputs`
//!
//! A parameter list is `key=VALUE;...` with VALUE `n:` (None), `i:`, `f:`,
//! `s:` (a string), `l:` (a Python list literal), `v:` (a vector, kept as a
//! string) or `c:<node>.<param>` (wired from another node). Edges are
//! `port:node,node|...`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use super::value::{Value, literal_eval};

/// A parameter: a literal, or a wire from another node's output.
#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    Lit(Value),
    Wire { node: usize, param: String },
}

#[derive(Clone, Debug)]
pub struct NodeData {
    pub guid: String,
    pub ty: String,
    pub atom: Option<String>,
    pub params: BTreeMap<String, Param>,
    pub atom_params: BTreeMap<String, Param>,
    pub edges: BTreeMap<String, Vec<usize>>,
}

#[derive(Clone, Debug)]
pub struct GraphData {
    pub id: i64,
    pub name: String,
    pub tags: Vec<String>,
    pub nodes: Vec<NodeData>,
    /// For each node, the nodes with an edge into it (`input_connections`).
    pub inputs: Vec<Vec<usize>>,
}

#[derive(Clone, Debug, Default)]
pub struct TypeData {
    pub in_ports: Vec<String>,
    pub defaults: BTreeMap<String, Value>,
}

/// Every graph, plus the node-type and atom defaults the engine falls back to.
#[derive(Clone, Debug, Default)]
pub struct Library {
    pub graphs: HashMap<i64, Arc<GraphData>>,
    pub types: HashMap<String, TypeData>,
    pub atoms: HashMap<String, BTreeMap<String, Value>>,
}

impl Library {
    pub fn graph(&self, id: i64) -> Option<Arc<GraphData>> {
        self.graphs.get(&id).cloned()
    }

    /// Whether a node of this type takes flow (`InPort.input`); a node that
    /// does not refreshes its own inputs when asked for its values.
    pub fn is_flow(&self, ty: &str) -> bool {
        self.types
            .get(ty)
            .is_some_and(|t| t.in_ports.iter().any(|p| p == "input"))
    }

    /// Load `dir/{graphs,nodes,nodetypes,atoms}.tsv`.
    pub fn load(dir: &Path) -> Result<Library, String> {
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name))
                .map_err(|e| format!("{}: {e}", dir.join(name).display()))
        };
        Library::parse(
            &read("graphs.tsv")?,
            &read("nodes.tsv")?,
            &read("nodetypes.tsv")?,
            &read("atoms.tsv")?,
        )
    }

    pub fn parse(graphs: &str, nodes: &str, types: &str, atoms: &str) -> Result<Library, String> {
        let mut lib = Library::default();
        for (n, line) in rows(types) {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 5 {
                return Err(format!("nodetypes.tsv line {n}: {} fields", f.len()));
            }
            lib.types.insert(
                unesc(f[1]),
                TypeData {
                    in_ports: list(f[2]),
                    defaults: literal_params(f[4])
                        .map_err(|e| format!("nodetypes.tsv line {n}: {e}"))?,
                },
            );
        }
        for (n, line) in rows(atoms) {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 4 {
                return Err(format!("atoms.tsv line {n}: {} fields", f.len()));
            }
            lib.atoms.insert(
                unesc(f[1]),
                literal_params(f[3]).map_err(|e| format!("atoms.tsv line {n}: {e}"))?,
            );
        }
        let mut meta: BTreeMap<i64, (String, Vec<String>)> = BTreeMap::new();
        for (n, line) in rows(graphs) {
            let f: Vec<&str> = line.split('\t').collect();
            let id = f
                .first()
                .and_then(|s| s.parse().ok())
                .ok_or(format!("graphs.tsv line {n}: no id"))?;
            meta.insert(
                id,
                (
                    unesc(f.get(1).unwrap_or(&"")),
                    list(f.get(2).unwrap_or(&"")),
                ),
            );
        }
        // Raw rows per graph first: edges and wires name GUIDs, resolved to
        // indexes once the graph's node list is complete.
        let mut raw: BTreeMap<i64, Vec<[String; 6]>> = BTreeMap::new();
        for (n, line) in rows(nodes) {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 7 {
                return Err(format!("nodes.tsv line {n}: {} fields", f.len()));
            }
            let gid: i64 = f[0]
                .parse()
                .map_err(|_| format!("nodes.tsv line {n}: bad graph id"))?;
            raw.entry(gid)
                .or_default()
                .push([f[1], f[2], f[3], f[4], f[5], f[6]].map(str::to_string));
        }
        for (gid, rows) in raw {
            let index: HashMap<String, usize> = rows
                .iter()
                .enumerate()
                .map(|(i, r)| (unesc(&r[0]), i))
                .collect();
            let mut nodes = Vec::with_capacity(rows.len());
            for r in &rows {
                let atom = if r[2] == "-" || r[2].is_empty() {
                    None
                } else {
                    Some(unesc(&r[2]))
                };
                let mut edges = BTreeMap::new();
                for part in r[5].split('|').filter(|p| !p.is_empty()) {
                    let (port, targets) = part.split_once(':').unwrap_or((part, ""));
                    let ids = targets
                        .split(',')
                        .filter(|t| !t.is_empty())
                        .filter_map(|t| index.get(&unesc(t)).copied())
                        .collect();
                    edges.insert(unesc(port), ids);
                }
                nodes.push(NodeData {
                    guid: unesc(&r[0]),
                    ty: unesc(&r[1]),
                    atom,
                    params: params(&r[3], &index).map_err(|e| format!("graph {gid}: {e}"))?,
                    atom_params: params(&r[4], &index).map_err(|e| format!("graph {gid}: {e}"))?,
                    edges,
                });
            }
            let mut inputs = vec![Vec::new(); nodes.len()];
            for (i, n) in nodes.iter().enumerate() {
                for t in n.edges.values().flatten() {
                    if !inputs[*t].contains(&i) {
                        inputs[*t].push(i);
                    }
                }
            }
            let (name, tags) = meta.get(&gid).cloned().unwrap_or_default();
            lib.graphs.insert(
                gid,
                Arc::new(GraphData {
                    id: gid,
                    name,
                    tags,
                    nodes,
                    inputs,
                }),
            );
        }
        Ok(lib)
    }
}

fn rows(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
        .map(|(i, l)| (i + 1, l))
}

fn list(field: &str) -> Vec<String> {
    field
        .split(',')
        .filter(|s| !s.is_empty())
        .map(unesc)
        .collect()
}

/// Undo the `%XX` escaping.
pub fn unesc(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h << 4) | l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn value(enc: &str) -> Result<Value, String> {
    let (tag, body) = enc
        .split_once(':')
        .ok_or(format!("value {enc:?} has no tag"))?;
    let body = unesc(body);
    Ok(match tag {
        "n" => Value::None,
        "i" => Value::Int(body.parse().map_err(|_| format!("bad int {body:?}"))?),
        "f" => Value::Float(body.parse().map_err(|_| format!("bad float {body:?}"))?),
        "s" | "v" => Value::Str(body),
        "l" => literal_eval(&body).unwrap_or(Value::Str(body)),
        other => return Err(format!("unknown value tag {other:?}")),
    })
}

fn split_params(field: &str) -> impl Iterator<Item = (String, &str)> {
    field
        .split(';')
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.split_once('=').map(|(k, v)| (unesc(k), v)))
}

fn literal_params(field: &str) -> Result<BTreeMap<String, Value>, String> {
    split_params(field)
        .map(|(k, v)| value(v).map(|v| (k, v)))
        .collect()
}

fn params(field: &str, index: &HashMap<String, usize>) -> Result<BTreeMap<String, Param>, String> {
    let mut out = BTreeMap::new();
    for (k, v) in split_params(field) {
        let p = if let Some(wire) = v.strip_prefix("c:") {
            let (node, param) = wire.split_once('.').unwrap_or((wire, ""));
            match index.get(&unesc(node)) {
                Some(&i) => Param::Wire {
                    node: i,
                    param: unesc(param),
                },
                // A wire to a node that is not in the graph reads None.
                None => Param::Lit(Value::None),
            }
        } else {
            Param::Lit(value(v)?)
        };
        out.insert(k, p);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TYPES: &str = "#type_id\tname\tin_ports\tout_ports\tdefaults\toutputs\n\
        1\tRoot\t\toutput\t\t\n\
        17\tDelay\tinput\toutput\tdelay=f:1.0\t\n";
    const ATOMS: &str =
        "#atom_id\tname\tatom_type\tdefaults\toutputs\n9\tAnswer\t4\tvalue=i:0\tvalue\n";
    const GRAPHS: &str = "#graph_id\tname\ttags\tparent\n7\tA%3B graph\tserver,test\t-\n";
    const NODES: &str = "#graph_id\tnode\ttype\tatom\tparams\tatom_params\tedges\n\
        7\tr\tRoot\t-\t\t\toutput:d\n\
        7\td\tDelay\t-\tdelay=f:2.5\t\toutput:\n\
        7\tg\tGetterNode\tAnswer\tkey=c:d.out;text=s:a%3Db\t\t\n";

    #[test]
    fn a_library_round_trips_the_bundle_format() {
        let lib = Library::parse(GRAPHS, NODES, TYPES, ATOMS).unwrap();
        let g = lib.graph(7).unwrap();
        assert_eq!(g.name, "A; graph");
        assert_eq!(g.tags, vec!["server", "test"]);
        assert_eq!(g.nodes[0].edges["output"], vec![1]);
        assert_eq!(g.inputs[1], vec![0]);
        assert_eq!(g.nodes[1].params["delay"], Param::Lit(Value::Float(2.5)));
        assert_eq!(
            g.nodes[2].params["key"],
            Param::Wire {
                node: 1,
                param: "out".into()
            }
        );
        assert_eq!(g.nodes[2].params["text"], Param::Lit(Value::str("a=b")));
        assert_eq!(g.nodes[2].atom.as_deref(), Some("Answer"));
        assert!(lib.is_flow("Delay") && !lib.is_flow("Root"));
        assert_eq!(lib.types["Delay"].defaults["delay"], Value::Float(1.0));
        assert_eq!(lib.atoms["Answer"]["value"], Value::Int(0));
    }
}
