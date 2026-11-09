//! "Play flow": an animated trace through the graph, starting at one symbol.
//!
//! Two ways to play it:
//! - **waves**: everything the start reaches, one level of depth at a time;
//! - **call and return**: depth-first in source order, the way the code runs. A call goes into
//!   the callee, which returns to the caller before the caller's next call. Rendering a child
//!   hands it to React: nothing returns to the parent. Handlers run afterwards, "on events".
//!
//! The graph is read from the source, not from running it: branches, loops and how deep a
//! recursion goes are unknown, so this is one plausible order and a recursion is shown once.

use crate::graph::{Edge, EdgeKind, Graph};
use std::collections::{HashMap, HashSet, VecDeque};

/// Where the trace begins.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Start {
    /// at the selected symbol
    Here,
    /// at whatever calls / renders the selected symbol, one hop up
    Parent,
    /// at an entry point, down the path to the selected symbol
    Entry,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Waves,
    CallReturn,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Move {
    /// the pulse travels `from` -> `to`
    Go,
    /// the pulse travels back `to` -> `from`: a return
    Back,
}

#[derive(Clone, Debug)]
pub struct Step {
    pub edge: Edge,
    pub mv: Move,
    /// start time (s)
    pub t0: f32,
    /// 0 = not animated (a rendered component handing control back to React)
    pub dur: f32,
    /// call depth (call and return) or wave (waves)
    pub depth: usize,
    /// the target is already on the stack: a recursion, not entered again
    pub recursion: bool,
    /// the target was already shown: not entered again
    pub repeat: bool,
    /// runs later, when the user triggers an event (handlers and what they call)
    pub later: bool,
    /// call stack once this step is done, bottom first (call and return only)
    pub stack: Vec<usize>,
}

/// Edges are unique per (from, to, kind) in the graph.
pub type EdgeKey = (usize, usize, EdgeKind);

pub fn key(e: &Edge) -> EdgeKey {
    (e.from, e.to, e.kind)
}

pub struct Trace {
    pub start: usize,
    pub style: Style,
    /// sorted by start time
    pub steps: Vec<Step>,
    /// lit at t = 0: where the trace begins
    pub origins: Vec<usize>,
    /// call stack before the first step
    pub stack0: Vec<usize>,
    /// every symbol and edge the trace touches
    pub nodes: HashSet<usize>,
    pub edges: HashSet<EdgeKey>,
    /// length (s)
    pub total: f32,
    /// stopped early: too many steps or too deep
    pub truncated: bool,
    /// the same trace laid out as a sequence diagram
    pub seq: Seq,
}

/// A trace as a sequence diagram: one lane (lifeline) per symbol, one row (arrow) per animated step.
#[derive(Default)]
pub struct Seq {
    /// symbols, in order of first appearance
    pub lanes: Vec<usize>,
    pub rows: Vec<SeqRow>,
    /// call stack (as lanes, bottom first) before the first row
    pub stack0: Vec<usize>,
}

pub struct SeqRow {
    pub step: usize,
    /// lanes the arrow leaves and reaches: a return goes from the callee back to the caller
    pub from: usize,
    pub to: usize,
    /// call stack (as lanes, bottom first) from this row until the next one
    pub stack: Vec<usize>,
}

impl Seq {
    /// Row of step `i`, if it is animated.
    pub fn row_of(&self, i: usize) -> Option<usize> {
        self.rows.binary_search_by_key(&i, |r| r.step).ok()
    }
}

/// What to draw at one moment of the trace.
#[derive(Default)]
pub struct Frame {
    /// pulses on their way: (step, eased progress 0..1)
    pub moving: Vec<(usize, f32)>,
    /// lit part of each edge's curve, measured from its start (0..1)
    pub trail: HashMap<EdgeKey, f32>,
    /// seconds since a pulse last arrived at a symbol
    pub pings: HashMap<usize, f32>,
    /// symbols a pulse has reached so far
    pub reached: HashSet<usize>,
    /// latest animated step that has started
    pub current: Option<usize>,
    /// call stack, bottom first (call and return only)
    pub stack: Vec<usize>,
}

const MAX_STEPS: usize = 400;
const MAX_WAVES: usize = 8;
const MAX_DEPTH: usize = 16;
const MAX_ANCESTORS: usize = 2000;

// timing (s)
const LEAD: f32 = 0.35;
const WAVE_DUR: f32 = 0.75;
const STAGGER: f32 = 0.18;
const WAVE_GAP: f32 = 0.12;
/// handlers run on an event, after the render: their pulses start a bit later
const HANDLER_DELAY: f32 = 0.5;
const CALL_DUR: f32 = 0.7;
const CALL_GAP: f32 = 0.15;
/// pause before the "on events" part of a call-and-return trace
const LATER_PAUSE: f32 = 0.6;
const TAIL: f32 = 0.5;

pub fn ease(p: f32) -> f32 {
    if p < 0.5 { 2.0 * p * p } else { 1.0 - (-2.0 * p + 2.0).powi(2) / 2.0 }
}

type Ok<'a> = &'a dyn Fn(EdgeKind) -> bool;

/// Edges leaving `n`, in source order. Handler edges are written in another component
/// (the one passing the prop), so they come last.
fn outs(g: &Graph, n: usize, ok: Ok) -> Vec<Edge> {
    let mut v: Vec<Edge> =
        g.out_edges(n).filter(|e| e.to != n && e.kind != EdgeKind::Context && ok(e.kind)).copied().collect();
    v.sort_by_key(|e| (e.site != e.from, e.line));
    v
}

/// Edges arriving at `n`, by caller file and line.
fn ins(g: &Graph, n: usize, ok: Ok) -> Vec<Edge> {
    let mut v: Vec<Edge> =
        g.in_edges(n).filter(|e| e.from != n && e.kind != EdgeKind::Context && ok(e.kind)).copied().collect();
    v.sort_by(|a, b| (&g.symbols[a.site].file, a.line).cmp(&(&g.symbols[b.site].file, b.line)));
    v
}

fn dedup(it: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut seen = HashSet::new();
    it.filter(|n| seen.insert(*n)).collect()
}

fn go(edge: Edge, depth: usize, repeat: bool) -> Step {
    Step {
        edge,
        mv: Move::Go,
        t0: 0.0,
        dur: 1.0,
        depth,
        recursion: false,
        repeat,
        later: edge.kind == EdgeKind::Handler,
        stack: Vec::new(),
    }
}

impl Trace {
    pub fn build(g: &Graph, start: usize, from: Start, style: Style, ok: Ok) -> Trace {
        let (mut steps, origins, stack0, truncated) = match style {
            Style::Waves => {
                let (s, o, t) = waves(g, start, from, ok);
                (s, o, Vec::new(), t)
            }
            Style::CallReturn => call_return(g, start, from, ok),
        };
        let total = match style {
            Style::Waves => time_waves(&mut steps),
            Style::CallReturn => time_calls(&mut steps),
        };
        let mut nodes: HashSet<usize> = origins.iter().copied().collect();
        nodes.insert(start);
        let mut edges = HashSet::new();
        for s in &steps {
            nodes.insert(s.edge.from);
            nodes.insert(s.edge.to);
            edges.insert(key(&s.edge));
        }
        let seq = sequence(&steps, &origins, &stack0);
        Trace { start, style, steps, origins, stack0, nodes, edges, total, truncated, seq }
    }

    /// Everything to draw at time `t`.
    pub fn frame(&self, t: f32) -> Frame {
        let mut f = Frame { stack: self.stack0.clone(), ..Frame::default() };
        for &o in &self.origins {
            f.reached.insert(o);
            f.pings.insert(o, t);
        }
        for (i, s) in self.steps.iter().enumerate() {
            if s.t0 > t {
                break;
            }
            let p = if s.dur <= 0.0 { 1.0 } else { ((t - s.t0) / s.dur).clamp(0.0, 1.0) };
            let q = ease(p);
            let end = t - (s.t0 + s.dur);
            match s.mv {
                Move::Go => {
                    f.trail.insert(key(&s.edge), q);
                    if p >= 1.0 {
                        f.reached.insert(s.edge.to);
                        f.pings.insert(s.edge.to, end);
                    }
                }
                // a rendered child hands back to React (dur 0): its edge stays lit
                Move::Back if s.dur > 0.0 => {
                    f.trail.insert(key(&s.edge), 1.0 - q);
                    if p >= 1.0 {
                        f.pings.insert(s.edge.from, end);
                    }
                }
                Move::Back => {}
            }
            if s.dur > 0.0 {
                if p < 1.0 {
                    f.moving.push((i, q));
                }
                f.current = Some(i);
            }
            // a call's frame appears when the pulse arrives; a return pops it right away
            if !(s.mv == Move::Go && p < 1.0) {
                f.stack.clone_from(&s.stack);
            }
        }
        f
    }

    /// End of the next animated step after `t` (for stepping forward).
    pub fn next_stop(&self, t: f32) -> f32 {
        self.stops().find(|&e| e > t + 1e-3).unwrap_or(self.total)
    }

    /// End of the previous animated step before `t` (for stepping back).
    pub fn prev_stop(&self, t: f32) -> f32 {
        self.stops().filter(|&e| e < t - 1e-3).last().unwrap_or(0.0)
    }

    fn stops(&self) -> impl Iterator<Item = f32> + '_ {
        self.steps.iter().filter(|s| s.dur > 0.0).map(|s| s.t0 + s.dur)
    }

    /// One line for step `i`: "ProductList calls useProducts".
    pub fn describe(&self, g: &Graph, i: usize) -> String {
        let s = &self.steps[i];
        let (a, b) = (&g.symbols[s.edge.from].name, &g.symbols[s.edge.to].name);
        let text = match (s.mv, s.edge.kind) {
            // the target is already on the stack
            (Move::Go, _) if s.recursion => format!("{a} calls {b} again (recursion)"),
            (Move::Go, EdgeKind::Calls) => format!("{a} calls {b}"),
            (Move::Go, EdgeKind::Renders) => format!("{a} renders {b}"),
            (Move::Go, EdgeKind::Handler) => format!("{a} runs {b} on an event"),
            (Move::Go, EdgeKind::Defines) => format!("{a} defines {b}"),
            (Move::Go, EdgeKind::Context) => format!("{a} provides context to {b}"),
            (Move::Back, _) => format!("{b} returns to {a}"),
        };
        if s.repeat && s.mv == Move::Go { format!("{text} (already shown)") } else { text }
    }
}

// ---------------------------------------------------------------- sequence diagram

fn sequence(steps: &[Step], origins: &[usize], stack0: &[usize]) -> Seq {
    let mut lane_of: HashMap<usize, usize> = HashMap::new();
    let mut lanes: Vec<usize> = Vec::new();
    let mut lane = |s: usize| {
        *lane_of.entry(s).or_insert_with(|| {
            lanes.push(s);
            lanes.len() - 1
        })
    };
    for &o in origins {
        lane(o);
    }
    let mut seq_stack0: Vec<usize> = stack0.iter().map(|&s| lane(s)).collect();
    let mut rows: Vec<SeqRow> = Vec::new();
    for (i, s) in steps.iter().enumerate() {
        let (a, b) = match s.mv {
            Move::Go => (s.edge.from, s.edge.to),
            Move::Back => (s.edge.to, s.edge.from),
        };
        let (from, to) = (lane(a), lane(b));
        let stack: Vec<usize> = s.stack.iter().map(|&x| lane(x)).collect();
        if s.dur > 0.0 {
            rows.push(SeqRow { step: i, from, to, stack });
        } else {
            // not animated (a render handing back to React): no arrow, only the stack changes
            match rows.last_mut() {
                Some(r) => r.stack = stack,
                None => seq_stack0 = stack,
            }
        }
    }
    Seq { lanes, rows, stack0: seq_stack0 }
}

// ---------------------------------------------------------------- waves

fn waves(g: &Graph, start: usize, from: Start, ok: Ok) -> (Vec<Step>, Vec<usize>, bool) {
    let mut steps = Vec::new();
    let mut origins = vec![start];
    let mut seen: HashSet<usize> = HashSet::from([start]);
    let mut w0 = 0;
    match from {
        Start::Here => {}
        Start::Parent => {
            let callers = ins(g, start, ok);
            if !callers.is_empty() {
                origins = dedup(callers.iter().map(|e| e.from));
                seen.extend(origins.iter().copied());
                steps.extend(callers.into_iter().map(|e| go(e, 0, false)));
                w0 = 1;
            }
        }
        Start::Entry => {
            let anc = ancestors(g, start, ok);
            let mut roots: Vec<usize> = anc
                .iter()
                .copied()
                .filter(|&n| n != start && !ins(g, n, ok).iter().any(|e| anc.contains(&e.from)))
                .collect();
            roots.sort();
            if !roots.is_empty() {
                // each ancestor at its distance from a root, along edges that stay among the ancestors
                let mut level: HashMap<usize, usize> = roots.iter().map(|&r| (r, 0)).collect();
                let mut queue: VecDeque<usize> = roots.iter().copied().collect();
                while let Some(n) = queue.pop_front() {
                    for e in outs(g, n, ok) {
                        if anc.contains(&e.to) && !level.contains_key(&e.to) {
                            level.insert(e.to, level[&n] + 1);
                            steps.push(go(e, level[&n], false));
                            queue.push_back(e.to);
                        }
                    }
                }
                w0 = level.get(&start).copied().unwrap_or(0);
                origins = roots;
                seen.extend(anc);
            }
        }
    }
    let mut frontier = vec![start];
    let mut wave = w0;
    let mut truncated = false;
    'waves: while !frontier.is_empty() {
        if wave >= w0 + MAX_WAVES {
            truncated = frontier.iter().any(|&n| !outs(g, n, ok).is_empty());
            break;
        }
        let mut next = Vec::new();
        for &n in &frontier {
            for e in outs(g, n, ok) {
                if steps.len() >= MAX_STEPS {
                    truncated = true;
                    break 'waves;
                }
                let repeat = !seen.insert(e.to);
                steps.push(go(e, wave, repeat));
                if !repeat {
                    next.push(e.to);
                }
            }
        }
        frontier = next;
        wave += 1;
    }
    (steps, origins, truncated)
}

/// `start` and everything with a path to it.
fn ancestors(g: &Graph, start: usize, ok: Ok) -> HashSet<usize> {
    let mut seen = HashSet::from([start]);
    let mut queue = VecDeque::from([start]);
    while let Some(n) = queue.pop_front() {
        for e in ins(g, n, ok) {
            if seen.len() < MAX_ANCESTORS && seen.insert(e.from) {
                queue.push_back(e.from);
            }
        }
    }
    seen
}

/// Waves play one after another; edges inside a wave fire in order, a little apart.
fn time_waves(steps: &mut [Step]) -> f32 {
    let mut by_wave: Vec<Vec<usize>> = Vec::new();
    for (i, s) in steps.iter().enumerate() {
        if by_wave.len() <= s.depth {
            by_wave.resize(s.depth + 1, Vec::new());
        }
        by_wave[s.depth].push(i);
    }
    let mut t = LEAD;
    for wave in by_wave.iter().filter(|w| !w.is_empty()) {
        // big waves: keep the whole wave under ~1.5 s
        let stagger = STAGGER.min(1.5 / wave.len() as f32);
        let mut end = t;
        for (k, &i) in wave.iter().enumerate() {
            let s = &mut steps[i];
            s.t0 = t + k as f32 * stagger + if s.later { HANDLER_DELAY } else { 0.0 };
            s.dur = WAVE_DUR;
            end = end.max(s.t0 + s.dur);
        }
        t = end + WAVE_GAP;
    }
    steps.sort_by(|a, b| a.t0.total_cmp(&b.t0));
    t + TAIL
}

// ---------------------------------------------------------------- call and return

struct Calls<'a> {
    g: &'a Graph,
    ok: Ok<'a>,
    steps: Vec<Step>,
    stack: Vec<usize>,
    /// symbols already run once: calling them again shows the call but doesn't go inside
    entered: HashSet<usize>,
    /// handler edges met on the way: they run after the render, on events
    later: VecDeque<Edge>,
    in_later: bool,
    truncated: bool,
}

impl Calls<'_> {
    fn push(&mut self, edge: Edge, mv: Move, animated: bool, depth: usize, recursion: bool, repeat: bool) {
        self.steps.push(Step {
            edge,
            mv,
            t0: 0.0,
            dur: if animated { 1.0 } else { 0.0 },
            depth,
            recursion,
            repeat,
            later: self.in_later || edge.kind == EdgeKind::Handler,
            stack: self.stack.clone(),
        });
    }

    /// Run `n` (on top of the stack): its calls first, then the components it renders.
    fn enter(&mut self, n: usize) {
        self.entered.insert(n);
        let outs = outs(self.g, n, self.ok);
        for e in outs.iter().filter(|e| e.kind == EdgeKind::Calls) {
            self.follow(*e);
        }
        self.later.extend(outs.iter().filter(|e| e.kind == EdgeKind::Handler));
        for e in outs.iter().filter(|e| e.kind == EdgeKind::Renders) {
            self.follow(*e);
        }
    }

    /// Go along `e` from the top of the stack, run its target, come back.
    fn follow(&mut self, e: Edge) {
        if self.steps.len() >= MAX_STEPS {
            self.truncated = true;
            return;
        }
        let depth = self.stack.len().saturating_sub(1);
        let recursion = self.stack.contains(&e.to);
        let too_deep = self.stack.len() >= MAX_DEPTH;
        self.truncated |= too_deep && !recursion;
        let repeat = !recursion && (too_deep || self.entered.contains(&e.to));
        self.stack.push(e.to);
        self.push(e, Move::Go, true, depth, recursion, repeat);
        if !recursion && !repeat {
            self.enter(e.to);
        }
        self.stack.pop();
        // a call returns to its caller; a rendered component goes back to React, not to its parent
        self.push(e, Move::Back, e.kind != EdgeKind::Renders, depth, false, false);
    }

    /// The handlers met during the render, each from the component that receives it.
    fn run_later(&mut self) {
        self.in_later = true;
        while let Some(e) = self.later.pop_front() {
            if self.steps.len() >= MAX_STEPS {
                self.truncated = true;
                break;
            }
            self.stack = vec![e.from];
            self.follow(e);
        }
    }
}

/// Shortest path (root first) from something nobody calls or renders down to `start`.
fn path_from_entry(g: &Graph, start: usize, ok: Ok) -> Option<Vec<Edge>> {
    let flows = |k: EdgeKind| k != EdgeKind::Defines && ok(k);
    let mut toward: HashMap<usize, Edge> = HashMap::new();
    let mut queue = VecDeque::from([start]);
    while let Some(n) = queue.pop_front() {
        let callers = ins(g, n, &flows);
        if callers.is_empty() {
            if n == start {
                return None;
            }
            let mut path = Vec::new();
            let mut cur = n;
            while cur != start {
                let e = toward[&cur];
                path.push(e);
                cur = e.to;
            }
            return Some(path);
        }
        for e in callers {
            if e.from != start && toward.len() < MAX_ANCESTORS && !toward.contains_key(&e.from) {
                toward.insert(e.from, e);
                queue.push_back(e.from);
            }
        }
    }
    None
}

fn call_return(g: &Graph, start: usize, from: Start, ok: Ok) -> (Vec<Step>, Vec<usize>, Vec<usize>, bool) {
    let mut c = Calls {
        g,
        ok,
        steps: Vec::new(),
        stack: vec![start],
        entered: HashSet::new(),
        later: VecDeque::new(),
        in_later: false,
        truncated: false,
    };
    let mut origins = vec![start];
    let callers: Vec<Edge> = match from {
        Start::Parent => ins(g, start, ok).into_iter().filter(|e| e.kind != EdgeKind::Defines).collect(),
        _ => Vec::new(),
    };
    let path = match from {
        Start::Entry => path_from_entry(g, start, ok),
        _ => None,
    };
    if let Some(path) = path {
        // down the path from the entry point, run `start`, then back up
        let root = path[0].from;
        origins = vec![root];
        c.stack = vec![root];
        c.entered.insert(root);
        for e in &path {
            let depth = c.stack.len() - 1;
            c.stack.push(e.to);
            c.entered.insert(e.to);
            c.push(*e, Move::Go, true, depth, false, false);
        }
        c.enter(start);
        for e in path.iter().rev() {
            c.stack.pop();
            let depth = c.stack.len() - 1;
            c.push(*e, Move::Back, e.kind != EdgeKind::Renders, depth, false, false);
        }
    } else if !callers.is_empty() {
        // each caller in turn: the first runs `start`, the others show the call only
        origins = dedup(callers.iter().map(|e| e.from));
        for e in callers {
            c.stack = vec![e.from];
            c.follow(e);
        }
    } else {
        c.enter(start);
    }
    let stack0 = vec![origins[0]];
    c.run_later();
    (c.steps, origins, stack0, c.truncated)
}

/// One step after another; the "on events" part starts after a short pause.
fn time_calls(steps: &mut [Step]) -> f32 {
    let mut t = LEAD;
    let mut in_later = false;
    for s in steps.iter_mut() {
        if s.later && !in_later {
            t += LATER_PAUSE;
            in_later = true;
        }
        s.t0 = t;
        if s.dur > 0.0 {
            s.dur = CALL_DUR;
            t += CALL_DUR + CALL_GAP;
        }
    }
    t + TAIL
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph;
    use std::path::Path;

    fn fixture() -> Graph {
        graph::build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flow"))
    }

    fn sym(g: &Graph, name: &str) -> usize {
        g.symbols.iter().position(|s| s.name == name).unwrap_or_else(|| panic!("no symbol {name}"))
    }

    fn all(_: EdgeKind) -> bool {
        true
    }

    /// Animated steps as "A > B" (go) / "A < B" (return); "↺" marks a recursion, "↑" a repeat.
    fn moves(g: &Graph, t: &Trace) -> Vec<String> {
        t.steps
            .iter()
            .filter(|s| s.dur > 0.0)
            .map(|s| {
                let (a, b) = (&g.symbols[s.edge.from].name, &g.symbols[s.edge.to].name);
                let arrow = if s.mv == Move::Go { ">" } else { "<" };
                let mark = if s.recursion { " ↺" } else if s.repeat { " ↑" } else { "" };
                format!("{a} {arrow} {b}{mark}")
            })
            .collect()
    }

    #[test]
    fn edges_keep_the_call_site_line() {
        let g = fixture();
        let (shop, card) = (sym(&g, "Shop"), sym(&g, "ProductCard"));
        let calls = g.out_edges(shop).find(|e| e.to == sym(&g, "useProducts")).unwrap();
        assert_eq!((calls.line, calls.site), (6, shop));
        let renders = g.out_edges(shop).find(|e| e.to == card).unwrap();
        assert_eq!(renders.line, 13);
        // <ProductCard onAdd={handleAdd} /> is written in Shop
        let handler = g.out_edges(card).find(|e| e.kind == EdgeKind::Handler).unwrap();
        assert_eq!((handler.to, handler.site, handler.line), (sym(&g, "handleAdd"), shop, 13));
    }

    #[test]
    fn call_and_return_goes_in_and_comes_back() {
        let g = fixture();
        let t = Trace::build(&g, sym(&g, "Shop"), Start::Here, Style::CallReturn, &all);
        assert_eq!(
            moves(&g, &t),
            [
                "Shop > useProducts",
                "useProducts > fetchJson",
                "useProducts < fetchJson",
                "Shop < useProducts",
                "Shop > ProductCard",
                "ProductCard > formatPrice",
                "ProductCard < formatPrice",
                // later, on an event
                "ProductCard > handleAdd",
                "handleAdd > addToCart",
                "addToCart > save",
                "addToCart < save",
                "handleAdd < addToCart",
                "ProductCard < handleAdd",
            ]
        );
        assert!(!t.truncated);
        // the render hands back to React without a pulse, then the handler part starts
        let first_later = t.steps.iter().position(|s| s.later).unwrap();
        let back = &t.steps[first_later - 1];
        assert_eq!((back.mv, back.edge.kind, back.dur), (Move::Back, EdgeKind::Renders, 0.0));
    }

    #[test]
    fn mutual_recursion_is_shown_once() {
        let g = fixture();
        let t = Trace::build(&g, sym(&g, "renderTree"), Start::Here, Style::CallReturn, &all);
        assert_eq!(
            moves(&g, &t),
            [
                "renderTree > renderChildren",
                "renderChildren > renderTree ↺",
                "renderChildren < renderTree",
                "renderTree < renderChildren",
            ]
        );
        // while the recursive call is there, the stack holds A, B, A
        let rec = t.steps.iter().find(|s| s.recursion).unwrap();
        let f = t.frame(rec.t0 + rec.dur + 0.01);
        let names: Vec<&str> = f.stack.iter().map(|&i| g.symbols[i].name.as_str()).collect();
        assert_eq!(names, ["renderTree", "renderChildren", "renderTree"]);
    }

    #[test]
    fn entry_walks_down_to_the_selection_and_back() {
        let g = fixture();
        let t = Trace::build(&g, sym(&g, "fetchJson"), Start::Entry, Style::CallReturn, &all);
        assert_eq!(
            moves(&g, &t),
            ["App > Shop", "Shop > useProducts", "useProducts > fetchJson", "useProducts < fetchJson", "Shop < useProducts"]
        );
        assert_eq!(t.origins, [sym(&g, "App")]);
    }

    #[test]
    fn parent_starts_at_each_caller() {
        let g = fixture();
        let t = Trace::build(&g, sym(&g, "formatPrice"), Start::Parent, Style::CallReturn, &all);
        // called from two places: the second call isn't entered again (nothing inside anyway)
        assert_eq!(
            moves(&g, &t),
            ["CartTotal > formatPrice", "CartTotal < formatPrice", "ProductCard > formatPrice ↑", "ProductCard < formatPrice"]
        );
        assert_eq!(t.origins.len(), 2);
    }

    #[test]
    fn waves_fire_level_by_level() {
        let g = fixture();
        let t = Trace::build(&g, sym(&g, "Shop"), Start::Here, Style::Waves, &all);
        let wave = |d: usize| -> Vec<String> {
            let mut v: Vec<String> = t
                .steps
                .iter()
                .filter(|s| s.depth == d)
                .map(|s| g.symbols[s.edge.to].name.clone() + if s.repeat { " ↑" } else { "" })
                .collect();
            v.sort();
            v
        };
        assert_eq!(wave(0), ["ProductCard", "handleAdd", "useProducts"]);
        assert_eq!(wave(1), ["addToCart", "fetchJson", "formatPrice", "handleAdd ↑"]);
        assert_eq!(wave(2), ["save"]);
        // the handler pulse waits for the render
        let h = t.steps.iter().find(|s| s.edge.kind == EdgeKind::Handler).unwrap();
        let first = t.steps.iter().filter(|s| s.depth == 1).map(|s| s.t0).fold(f32::MAX, f32::min);
        assert!(h.t0 >= first + HANDLER_DELAY);
    }

    #[test]
    fn sequence_has_a_lane_per_symbol_and_a_row_per_arrow() {
        let g = fixture();
        let t = Trace::build(&g, sym(&g, "Shop"), Start::Here, Style::CallReturn, &all);
        let s = &t.seq;
        let names: Vec<&str> = s.lanes.iter().map(|&i| g.symbols[i].name.as_str()).collect();
        assert_eq!(
            names,
            ["Shop", "useProducts", "fetchJson", "ProductCard", "formatPrice", "handleAdd", "addToCart", "save"]
        );
        assert_eq!(s.rows.len(), moves(&g, &t).len());
        // "useProducts < fetchJson": the return arrow goes from fetchJson's lane to useProducts'
        let r = &s.rows[2];
        assert_eq!((r.from, r.to, t.steps[r.step].mv), (2, 1, Move::Back));
        assert_eq!(r.stack, [0, 1]);
        // the render handing back to React (no arrow) leaves only Shop on the stack
        assert_eq!(s.rows[6].stack, [0]);
        assert_eq!(s.row_of(s.rows[4].step), Some(4));
    }

    #[test]
    fn sequence_recursion_stacks_the_same_lane_twice() {
        let g = fixture();
        let t = Trace::build(&g, sym(&g, "renderTree"), Start::Here, Style::CallReturn, &all);
        let s = &t.seq;
        assert_eq!(s.lanes.len(), 2);
        assert_eq!((s.rows[1].from, s.rows[1].to), (1, 0));
        assert_eq!(s.rows[1].stack, [0, 1, 0]);
        assert_eq!(s.stack0, [0]);
    }

    #[test]
    fn frame_lights_trails_and_returns_pull_them_back() {
        let g = fixture();
        let t = Trace::build(&g, sym(&g, "Shop"), Start::Here, Style::CallReturn, &all);
        let call = &t.steps[0];
        let k = key(&call.edge);
        // half-way through the call: the pulse is moving, the callee isn't on the stack yet
        let f = t.frame(call.t0 + call.dur / 2.0);
        assert_eq!(f.moving.len(), 1);
        assert!((f.trail[&k] - 0.5).abs() < 0.01);
        assert_eq!(f.stack, [sym(&g, "Shop")]);
        // after the matching return, the trail is gone and Shop is alone again
        let ret = t.steps.iter().find(|s| s.mv == Move::Back && key(&s.edge) == k).unwrap();
        let f = t.frame(ret.t0 + ret.dur + 0.01);
        assert_eq!(f.trail[&k], 0.0);
        assert_eq!(f.stack, [sym(&g, "Shop")]);
        assert!(f.reached.contains(&sym(&g, "fetchJson")));
        // stepping lands on step ends
        assert_eq!(t.next_stop(0.0), call.t0 + call.dur);
        assert_eq!(t.prev_stop(call.t0 + call.dur), 0.0);
    }
}
