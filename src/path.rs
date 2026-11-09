//! Shortest render / call path between two symbols ("how does A end up running B?").

use crate::graph::{Edge, EdgeKind, Graph};
use std::collections::VecDeque;

/// A route through the graph.
#[derive(Clone, Debug)]
pub struct Route {
    /// symbols in order, start first
    pub nodes: Vec<usize>,
    /// the links between them (`nodes.len() - 1` of them)
    pub edges: Vec<Edge>,
    /// the route runs from the second symbol picked to the first: `b` reaches `a`, not the other way
    pub reversed: bool,
}

/// Links that make code run: what a component renders, what a function calls, and the handlers a
/// component passes on. `Defines` and `Context` links are not steps of execution.
fn steps(k: EdgeKind) -> bool {
    matches!(k, EdgeKind::Renders | EdgeKind::Calls | EdgeKind::Handler)
}

/// Fewest-steps way from `from` to `to` following the direction of the links.
fn bfs(g: &Graph, from: usize, to: usize) -> Option<(Vec<usize>, Vec<Edge>)> {
    let n = g.symbols.len();
    let mut via: Vec<Option<Edge>> = vec![None; n];
    let mut seen = vec![false; n];
    seen[from] = true;
    let mut queue = VecDeque::from([from]);
    while let Some(i) = queue.pop_front() {
        if i == to {
            break;
        }
        for e in g.out_edges(i).filter(|e| steps(e.kind) && e.from != e.to) {
            if !seen[e.to] {
                seen[e.to] = true;
                via[e.to] = Some(*e);
                queue.push_back(e.to);
            }
        }
    }
    if !seen[to] {
        return None;
    }
    let mut edges = Vec::new();
    let mut at = to;
    while let Some(e) = via[at] {
        edges.push(e);
        at = e.from;
    }
    edges.reverse();
    let mut nodes = vec![from];
    nodes.extend(edges.iter().map(|e| e.to));
    Some((nodes, edges))
}

/// The shortest route between `a` and `b`. When `a` doesn't reach `b`, the other direction is tried.
pub fn find(g: &Graph, a: usize, b: usize) -> Option<Route> {
    if a == b {
        return None;
    }
    if let Some((nodes, edges)) = bfs(g, a, b) {
        return Some(Route { nodes, edges, reversed: false });
    }
    bfs(g, b, a).map(|(nodes, edges)| Route { nodes, edges, reversed: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build;
    use std::path::Path;

    fn fixture() -> Graph {
        build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/paths"))
    }

    fn id(g: &Graph, name: &str) -> usize {
        g.symbols.iter().position(|s| s.name == name).unwrap_or_else(|| panic!("no symbol {name}"))
    }

    fn names(g: &Graph, r: &Route) -> Vec<String> {
        r.nodes.iter().map(|&i| g.symbols[i].name.clone()).collect()
    }

    #[test]
    fn shortest_route_wins() {
        let g = fixture();
        // App -> Header -> Logo (2 steps) beats App -> Footer -> Legal -> Logo (3 steps)
        let r = find(&g, id(&g, "App"), id(&g, "Logo")).unwrap();
        assert_eq!(names(&g, &r), ["App", "Header", "Logo"]);
        assert!(!r.reversed);
        assert_eq!(r.edges.len(), 2);
        assert!(r.edges.iter().all(|e| e.kind == EdgeKind::Renders));
    }

    #[test]
    fn routes_mix_renders_and_calls() {
        let g = fixture();
        let r = find(&g, id(&g, "App"), id(&g, "round")).unwrap();
        assert_eq!(names(&g, &r), ["App", "Feed", "Card", "formatPrice", "round"]);
        let kinds: Vec<_> = r.edges.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, [EdgeKind::Renders, EdgeKind::Renders, EdgeKind::Calls, EdgeKind::Calls]);
        // each link starts where the previous one ended
        for (w, e) in r.nodes.windows(2).zip(&r.edges) {
            assert_eq!((e.from, e.to), (w[0], w[1]));
        }
    }

    #[test]
    fn picking_the_end_first_finds_the_route_the_other_way() {
        let g = fixture();
        let r = find(&g, id(&g, "Logo"), id(&g, "App")).unwrap();
        assert!(r.reversed);
        assert_eq!(names(&g, &r), ["App", "Header", "Logo"]);
    }

    #[test]
    fn unrelated_symbols_and_the_same_symbol_have_no_route() {
        let g = fixture();
        assert!(find(&g, id(&g, "App"), id(&g, "Island")).is_none());
        assert!(find(&g, id(&g, "Feed"), id(&g, "Header")).is_none());
        assert!(find(&g, id(&g, "App"), id(&g, "App")).is_none());
    }
}
