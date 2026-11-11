//! "Group by file": the symbol graph folded into one card per source file. Links between two
//! symbols become one link between their files (links inside a file disappear), so the canvas
//! shows how files use each other instead of how components do.

use crate::graph::{Edge, EdgeKind, Graph, SymKind};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct FileGraph {
    /// One symbol per source file that defines anything: its name is the file name, its kind
    /// the main thing the file holds (a component, else a hook, else a function).
    pub graph: Graph,
    /// file card of every symbol of the source graph
    pub of_sym: Vec<usize>,
    /// top-level symbols of each file card, in source order
    pub members: Vec<Vec<usize>>,
    /// every symbol of each file card, nested ones too
    pub all: Vec<Vec<usize>>,
    /// top-level symbols as the card lists them: the one named like the file first, then
    /// components, hooks and functions, each in source order
    pub chips: Vec<Vec<usize>>,
    /// file pairs where something links to a component: what a file opens into with "Hooks & functions" off
    pub comp_links: HashSet<(usize, usize)>,
    /// file card by path
    pub by_path: HashMap<String, usize>,
    /// symbol links behind each file link, per kind (same index as `graph.edges`)
    pub weight: Vec<Vec<(EdgeKind, usize)>>,
}

impl FileGraph {
    /// "renders 3 · calls 1": the symbol links from file card `from` to file card `to`.
    pub fn summary(&self, from: usize, to: usize) -> String {
        let mut w: Vec<(EdgeKind, usize)> = Vec::new();
        for (k, e) in self.graph.edges.iter().enumerate() {
            if e.from == from && e.to == to {
                for &(kind, n) in &self.weight[k] {
                    match w.iter_mut().find(|(x, _)| *x == kind) {
                        Some((_, m)) => *m += n,
                        None => w.push((kind, n)),
                    }
                }
            }
        }
        w.sort_by_key(|(k, _)| rank(*k));
        w.iter().map(|(k, n)| format!("{} {n}", k.label())).collect::<Vec<_>>().join(" · ")
    }
}

/// Rank of a link kind when several stand behind one file link: the strongest one is drawn.
fn rank(k: EdgeKind) -> u8 {
    match k {
        EdgeKind::Renders => 0,
        EdgeKind::Calls => 1,
        EdgeKind::Handler => 2,
        EdgeKind::Context => 3,
        EdgeKind::Defines => 4,
    }
}

pub fn build(g: &Graph) -> FileGraph {
    // files that define something, in path order
    let mut paths: Vec<&str> = g.symbols.iter().map(|s| s.file.as_str()).collect();
    paths.sort_unstable();
    paths.dedup();
    let by_path: HashMap<String, usize> = paths.iter().enumerate().map(|(i, p)| (p.to_string(), i)).collect();
    let of_sym: Vec<usize> = g.symbols.iter().map(|s| by_path[&s.file]).collect();
    let mut members = vec![Vec::new(); paths.len()];
    let mut all = vec![Vec::new(); paths.len()];
    for (i, s) in g.symbols.iter().enumerate() {
        all[of_sym[i]].push(i);
        if s.parent.is_none() {
            members[of_sym[i]].push(i);
        }
    }
    for m in members.iter_mut().chain(all.iter_mut()) {
        m.sort_by_key(|&i| g.symbols[i].line);
    }
    let chips: Vec<Vec<usize>> = members
        .iter()
        .enumerate()
        .map(|(f, m)| {
            let stem = paths[f].rsplit('/').next().unwrap_or(paths[f]).split('.').next().unwrap_or("");
            let mut c = m.clone();
            c.sort_by_key(|&i| {
                let s = &g.symbols[i];
                let kind = match s.kind {
                    SymKind::Component => 0,
                    SymKind::Hook => 1,
                    SymKind::Function => 2,
                };
                (!s.name.eq_ignore_ascii_case(stem), kind, s.line)
            });
            c
        })
        .collect();

    let symbols = paths
        .iter()
        .enumerate()
        .map(|(f, path)| {
            let tops = &members[f];
            let pick = |k: SymKind| tops.iter().any(|&i| g.symbols[i].kind == k);
            let kind = if pick(SymKind::Component) {
                SymKind::Component
            } else if pick(SymKind::Hook) {
                SymKind::Hook
            } else {
                SymKind::Function
            };
            // a file always has a symbol (that is how it got here): start from its first one
            let mut s = g.symbols[all[f][0]].clone();
            s.name = path.rsplit('/').next().unwrap_or(path).to_string();
            s.kind = kind;
            s.file = path.to_string();
            s.line = 1;
            s.end_line = all[f].iter().map(|&i| g.symbols[i].end_line).max().unwrap_or(1);
            s.parent = None;
            s.wraps.clear();
            s.deps = None;
            s.dep_hooks.clear();
            s.prop_hints.clear();
            s.drill_hints.clear();
            s.is_value = false;
            s.similar.clear();
            s.cycle = None;
            s.tests.clear();
            s.lazy = tops.iter().any(|&i| g.symbols[i].lazy);
            s.provides_suspense = false;
            s.provides_boundary = false;
            s.cover_hints.clear();
            s.lint_hints.clear();
            s.data = all[f].iter().flat_map(|&i| g.symbols[i].data.iter().cloned()).collect();
            s.env = tops.iter().find_map(|&i| g.symbols[i].env);
            s.entry = tops.iter().find_map(|&i| g.symbols[i].entry.clone());
            s.next_hints.clear();
            s.func = None;
            s
        })
        .collect();

    // one link per (from file, to file, context or not); context links stay apart so they keep their look
    let mut at: HashMap<(usize, usize, bool), usize> = HashMap::new();
    let mut edges: Vec<Edge> = Vec::new();
    let mut weight: Vec<Vec<(EdgeKind, usize)>> = Vec::new();
    let mut comp_links = HashSet::new();
    for e in g.edges.iter().filter(|e| e.kind != EdgeKind::Defines) {
        let (a, b) = (of_sym[e.from], of_sym[e.to]);
        if a == b {
            continue;
        }
        let ctx = e.kind == EdgeKind::Context;
        if !ctx && g.symbols[e.to].kind == SymKind::Component {
            comp_links.insert((a, b));
        }
        let k = *at.entry((a, b, ctx)).or_insert_with(|| {
            edges.push(Edge { from: a, to: b, site: a, ..*e });
            weight.push(Vec::new());
            edges.len() - 1
        });
        let fe = &mut edges[k];
        if rank(e.kind) < rank(fe.kind) {
            fe.kind = e.kind;
            fe.line = e.line;
        }
        fe.route |= e.route;
        let w = &mut weight[k];
        match w.iter_mut().find(|(kk, _)| *kk == e.kind) {
            Some((_, n)) => *n += 1,
            None => w.push((e.kind, 1)),
        }
    }
    for w in weight.iter_mut() {
        w.sort_by_key(|(k, _)| rank(*k));
    }

    let file_list = paths.iter().map(|p| p.to_string()).collect();
    let mut graph = Graph::from_parts(symbols, edges, file_list);
    graph.contexts = g.contexts.clone();
    FileGraph { graph, of_sym, members, all, chips, comp_links, by_path, weight }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn complex() -> (Graph, FileGraph) {
        let g = crate::graph::build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../complex-react"));
        let fg = build(&g);
        (g, fg)
    }

    #[test]
    fn every_symbol_lands_in_its_file_card() {
        let (g, fg) = complex();
        if g.symbols.is_empty() {
            return; // the sample project isn't next to this checkout
        }
        assert_eq!(fg.of_sym.len(), g.symbols.len());
        for (i, s) in g.symbols.iter().enumerate() {
            assert_eq!(fg.graph.symbols[fg.of_sym[i]].file, s.file);
        }
        assert_eq!(fg.all.iter().map(Vec::len).sum::<usize>(), g.symbols.len());
        // no file links to itself, and every link stands for at least one symbol link
        for (k, e) in fg.graph.edges.iter().enumerate() {
            assert_ne!(e.from, e.to);
            assert!(!fg.weight[k].is_empty());
        }
    }

    #[test]
    fn links_between_files_are_merged() {
        let (g, fg) = complex();
        let Some(&app) = fg.by_path.get("src/App.tsx") else { return };
        // App.tsx renders things from other files: one link per target file (and per context-ness)
        let mut seen = HashSet::new();
        for e in fg.graph.out_edges(app) {
            assert!(seen.insert((e.to, e.kind == EdgeKind::Context)), "one link per file pair");
        }
        assert!(!seen.is_empty());
        // and they are the symbol links of App.tsx that leave the file
        let leaving: HashSet<usize> = g
            .edges
            .iter()
            .filter(|e| e.kind != EdgeKind::Defines && fg.of_sym[e.from] == app && fg.of_sym[e.to] != app)
            .map(|e| fg.of_sym[e.to])
            .collect();
        let drawn: HashSet<usize> = fg.graph.out_edges(app).map(|e| e.to).collect();
        assert_eq!(leaving, drawn);
    }
}
