//! Boundary coverage: lazy chunks that nothing suspends, and route pages nothing catches errors for.
//!
//! Every `Renders` edge knows whether it is written inside a `<Suspense>` and inside an error
//! boundary (`Edge::suspended`, `Edge::caught`; a component that renders its `children` inside one
//! passes that on to what it wraps). Here those flags are followed from the entry points down:
//! a symbol is uncovered when some way of reaching it has no boundary on it.
//!
//! What can't be seen: a boundary around `render(<App />)` or `<RouterProvider>` outside any
//! component. A router shown inside a boundary (`<ErrorBoundary><Routes /></ErrorBoundary>`) is
//! noticed, and then its pages count as covered.

use crate::graph::{Edge, EdgeKind, Hint, SymKind, Symbol};
use std::collections::VecDeque;

/// Add `Symbol::cover_hints`. `host_suspended` / `host_caught`: a router was found inside a
/// Suspense / an error boundary, which covers every page it shows.
pub fn add_hints(symbols: &mut [Symbol], edges: &[Edge], host_suspended: bool, host_caught: bool) {
    let n = symbols.len();
    let renders: Vec<&Edge> = edges.iter().filter(|e| e.kind == EdgeKind::Renders && e.from != e.to).collect();
    let mut out: Vec<Vec<&Edge>> = vec![Vec::new(); n];
    let mut has_parent = vec![false; n];
    for e in &renders {
        out[e.from].push(e);
        has_parent[e.to] = true;
    }
    // entry points: top-level components nobody renders. A module-level route config (a symbol with
    // no function of its own) is shown by a router that isn't visible from here, so what it is
    // inside is unknown.
    let roots: Vec<usize> =
        (0..n).filter(|&i| symbols[i].parent.is_none() && symbols[i].kind == SymKind::Component && !has_parent[i]).collect();
    let is_config = |i: usize| symbols[i].func.is_none();

    // `reach[i]`: some way down from an entry point gets to `i` without passing the boundary.
    // `via[i]`: the edge that first did.
    let reach = |covered: &dyn Fn(&Edge) -> bool, seed: &dyn Fn(usize) -> bool| -> (Vec<bool>, Vec<Option<Edge>>) {
        let mut seen = vec![false; n];
        let mut via: Vec<Option<Edge>> = vec![None; n];
        let mut queue: VecDeque<usize> = VecDeque::new();
        for &r in &roots {
            if seed(r) {
                seen[r] = true;
                queue.push_back(r);
            }
        }
        while let Some(i) = queue.pop_front() {
            for e in &out[i] {
                if !covered(e) && !seen[e.to] {
                    seen[e.to] = true;
                    via[e.to] = Some(**e);
                    queue.push_back(e.to);
                }
            }
        }
        (seen, via)
    };

    // lazy chunks without a Suspense above (a router shown inside one covers its pages)
    {
        let (open, via) = reach(&|e| e.suspended || (host_suspended && e.route), &|r| !is_config(r));
        let hits: Vec<usize> = (0..n).filter(|&i| symbols[i].lazy && open[i]).collect();
        for i in hits {
            let by = via[i].map(|e| symbols[e.from].name.clone());
            let text = match by {
                Some(p) => format!(
                    "`{}` is loaded with lazy() but `{p}` renders it with no <Suspense> above it, so it suspends the nearest boundary higher up (or the whole app). Wrap it in a <Suspense fallback={{...}}>.",
                    symbols[i].name
                ),
                None => format!(
                    "`{}` is loaded with lazy() but nothing above it is a <Suspense>, so it suspends the nearest boundary higher up (or the whole app). Wrap it in a <Suspense fallback={{...}}>.",
                    symbols[i].name
                ),
            };
            symbols[i].cover_hints.push(Hint { warn: true, text });
        }
    }

    // route pages without an error boundary: not on their route (`errorElement`), not around them in JSX
    if !host_caught {
        let (open, _) = reach(&|e| e.caught, &|_| true);
        let mut flagged: Vec<(usize, String)> = Vec::new();
        for e in renders.iter().filter(|e| e.route && !e.caught && open[e.from]) {
            let text = format!(
                "`{}` is a route page with no error boundary: an error while rendering it isn't caught by an `errorElement` on its route or an ErrorBoundary around it, so it takes down everything above. Add an `errorElement` to the route, or wrap it in an ErrorBoundary.",
                symbols[e.to].name
            );
            if !flagged.iter().any(|(t, _)| *t == e.to) {
                flagged.push((e.to, text));
            }
        }
        for (t, text) in flagged {
            symbols[t].cover_hints.push(Hint { warn: true, text });
        }
    }
}
