//! Prop drilling: a value handed down through components that never use it themselves.
//!
//! Two steps. While a component is parsed, `analyze` records every `<Child prop={value} />` that
//! forwards one of the component's own bindings, and which props the component only forwards
//! (reads nowhere else). Once the render edges are resolved, `find_chains` follows those forwards
//! from component to component and reports the long ones.

use crate::graph::{pattern_names, strip_expr, text, Symbol};
use std::collections::{HashMap, HashSet};
use tree_sitter::Node;

/// Components that only forward a prop before a chain counts as drilling.
pub const MIN_THROUGH: usize = 2;

/// `<Child child_prop={own} />` written in a component, before `Child` is resolved.
#[derive(Clone, Debug)]
pub struct Pass {
    /// the element's tag, as written
    pub child: String,
    /// 1-based line of the element (matches the render reference)
    pub elem_line: usize,
    /// the component's own binding (or `props.x`) that is handed on
    pub own: String,
    /// the prop it arrives as in the child
    pub child_prop: String,
    /// 1-based line of the attribute
    pub line: usize,
}

/// What one component does with its props.
#[derive(Clone, Debug, Default)]
pub struct Forwards {
    pub passes: Vec<Pass>,
    /// props that are only handed to child components: never read, called or rendered here
    pub pure: HashSet<String>,
}

/// A resolved `<Child prop={...} />`: `own` in `from` arrives as `child_prop` in `to`.
#[derive(Clone, Debug)]
pub struct Hop {
    pub from: usize,
    pub to: usize,
    pub own: String,
    pub child_prop: String,
    pub line: usize,
}

/// A prop travelling through components that only forward it.
#[derive(Clone, Debug)]
pub struct Chain {
    /// where the value comes from (state, a hook, a prop it also uses itself)
    pub origin: usize,
    /// components that only forward it, in order
    pub through: Vec<usize>,
    /// the first component that does something else with it
    pub end: usize,
    /// the hops it travels, in order (indices into the hop list)
    pub path: Vec<usize>,
}

/// A hint on one forwarding component.
#[derive(Clone, Debug)]
pub struct DrillHint {
    pub prop: String,
    pub text: String,
    /// 1-based line of the attribute that forwards the prop
    pub line: usize,
    /// where the prop is finally used
    pub end: usize,
}

struct Walk<'a> {
    src: &'a str,
    /// every name declared in the component's body or parameters
    tracked: &'a HashSet<String>,
    /// `props` in `function Card(props)`
    props_obj: Option<String>,
    /// reads that are not a plain forward, per prop
    uses: HashMap<String, usize>,
    /// forwards, per prop
    forwarded: HashSet<String>,
    /// `props` itself is read (spread, passed whole, ...): any prop may be used
    whole_props: bool,
    passes: Vec<Pass>,
}

impl Walk<'_> {
    /// The prop or binding this expression is: `user`, or `props.user`.
    fn key(&self, e: Node) -> Option<String> {
        let e = strip_expr(e);
        match e.kind() {
            "identifier" => {
                let n = text(e, self.src);
                (self.tracked.contains(n) && self.props_obj.as_deref() != Some(n)).then(|| n.to_string())
            }
            "member_expression" => {
                let (obj, prop) = (e.child_by_field_name("object")?, e.child_by_field_name("property")?);
                let is_props = obj.kind() == "identifier" && self.props_obj.as_deref() == Some(text(obj, self.src));
                (is_props && prop.kind() == "property_identifier").then(|| text(prop, self.src).to_string())
            }
            _ => None,
        }
    }

    fn element(&mut self, el: Node) {
        let Some(name) = el.child_by_field_name("name").filter(|n| n.kind() == "identifier") else { return };
        let child = text(name, self.src);
        if !child.starts_with(|c: char| c.is_uppercase()) {
            return;
        }
        let mut c = el.walk();
        for a in el.children(&mut c).filter(|a| a.kind() == "jsx_attribute") {
            let Some(prop) = a.named_child(0).filter(|p| p.kind() == "property_identifier").map(|p| text(p, self.src)) else {
                continue;
            };
            if matches!(prop, "key" | "ref") {
                continue;
            }
            let mut c2 = a.walk();
            let Some(e) = a.children(&mut c2).find(|k| k.kind() == "jsx_expression") else { continue };
            if e.named_child_count() != 1 {
                continue;
            }
            let Some(own) = self.key(e.named_child(0).unwrap()) else { continue };
            self.forwarded.insert(own.clone());
            self.passes.push(Pass {
                child: child.to_string(),
                elem_line: el.start_position().row + 1,
                own,
                child_prop: prop.to_string(),
                line: a.start_position().row + 1,
            });
        }
    }

    fn walk(&mut self, n: Node) {
        match n.kind() {
            "type_annotation" | "type_arguments" | "type_parameters" | "jsx_closing_element" => return,
            "jsx_opening_element" | "jsx_self_closing_element" => {
                let before = self.passes.len();
                self.element(n);
                // forwarded attributes are not reads: walk everything else on the element
                let mut c = n.walk();
                let kids: Vec<Node> = n.children(&mut c).collect();
                for k in kids {
                    if !(k.kind() == "jsx_attribute" && self.is_forward(k, before)) {
                        self.walk(k);
                    }
                }
                return;
            }
            "member_expression" => {
                if let (Some(obj), Some(prop)) = (n.child_by_field_name("object"), n.child_by_field_name("property")) {
                    if obj.kind() == "identifier"
                        && self.props_obj.as_deref() == Some(text(obj, self.src))
                        && prop.kind() == "property_identifier"
                    {
                        *self.uses.entry(text(prop, self.src).to_string()).or_default() += 1;
                        return;
                    }
                }
            }
            "identifier" | "shorthand_property_identifier" => {
                let t = text(n, self.src);
                if self.props_obj.as_deref() == Some(t) {
                    self.whole_props = true;
                } else if self.tracked.contains(t) {
                    *self.uses.entry(t.to_string()).or_default() += 1;
                }
                return;
            }
            _ => {}
        }
        let mut c = n.walk();
        let kids: Vec<Node> = n.children(&mut c).collect();
        for k in kids {
            self.walk(k);
        }
    }

    /// Is this attribute one of the forwards `element` just recorded (from index `since`)?
    fn is_forward(&self, attr: Node, since: usize) -> bool {
        let line = attr.start_position().row + 1;
        let Some(prop) = attr.named_child(0).map(|p| text(p, self.src)) else { return false };
        self.passes[since..].iter().any(|p| p.line == line && p.child_prop == prop)
    }
}

/// How a component uses its props. `func` is the component's function node; `bindings` the names
/// declared directly in its body and parameters.
pub fn analyze(func: Node, body: Node, bindings: &HashSet<String>, src: &str) -> Forwards {
    // `function Card(props)` => props object; `function Card({ a, b })` => destructured props
    let mut props_obj = None;
    let mut destructured = Vec::new();
    let first = func.child_by_field_name("parameter").or_else(|| {
        func.child_by_field_name("parameters").and_then(|ps| {
            let mut c = ps.walk();
            ps.named_children(&mut c).next()
        })
    });
    if let Some(p) = first {
        let pat = match p.kind() {
            "required_parameter" | "optional_parameter" => p.child_by_field_name("pattern"),
            _ => Some(p),
        };
        match pat {
            Some(pt) if pt.kind() == "identifier" => props_obj = Some(text(pt, src).to_string()),
            Some(pt) => pattern_names(pt, src, &mut destructured),
            None => {}
        }
    }
    let mut w = Walk {
        src,
        tracked: bindings,
        props_obj,
        uses: HashMap::new(),
        forwarded: HashSet::new(),
        whole_props: false,
        passes: Vec::new(),
    };
    w.walk(body);
    let is_prop = |n: &String| w.props_obj.is_some() || destructured.contains(n);
    let pure = w
        .forwarded
        .iter()
        .filter(|n| is_prop(n) && w.uses.get(*n).copied().unwrap_or(0) == 0 && !w.whole_props)
        .cloned()
        .collect();
    Forwards { passes: w.passes, pure }
}

/// Follow forwards from component to component and collect the chains with at least
/// `MIN_THROUGH` components that only pass the prop on.
pub fn find_chains(symbols: &[Symbol], hops: &[Hop]) -> Vec<Chain> {
    let pure = |c: usize, p: &str| symbols[c].forwards.as_ref().is_some_and(|f| f.pure.contains(p));
    let mut by_from: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, h) in hops.iter().enumerate() {
        by_from.entry(h.from).or_default().push(i);
    }
    // a hop continues an earlier one when its sender only forwards what it received
    let continues = |h: &Hop| hops.iter().any(|p| p.to == h.from && p.child_prop == h.own) && pure(h.from, &h.own);

    // the longest way on from hop `i`, as a list of hops
    fn longest(i: usize, hops: &[Hop], by_from: &HashMap<usize, Vec<usize>>, pure: &dyn Fn(usize, &str) -> bool, seen: &mut Vec<usize>) -> Vec<usize> {
        let h = &hops[i];
        let mut best = Vec::new();
        for &j in by_from.get(&h.to).into_iter().flatten() {
            let n = &hops[j];
            if n.own == h.child_prop && pure(h.to, &n.own) && !seen.contains(&n.to) {
                seen.push(n.to);
                let rest = longest(j, hops, by_from, pure, seen);
                seen.pop();
                if rest.len() + 1 > best.len() {
                    best = std::iter::once(j).chain(rest).collect();
                }
            }
        }
        best
    }

    let mut out = Vec::new();
    for (i, h) in hops.iter().enumerate() {
        if continues(h) {
            continue;
        }
        let mut seen = vec![h.from, h.to];
        let rest = longest(i, hops, &by_from, &pure, &mut seen);
        let path: Vec<usize> = std::iter::once(i).chain(rest).collect();
        // the sender counts when it only forwards a prop it was given
        let mut through: Vec<usize> = Vec::new();
        if pure(h.from, &h.own) {
            through.push(h.from);
        }
        through.extend(path[..path.len() - 1].iter().map(|&k| hops[k].to));
        if through.len() >= MIN_THROUGH {
            out.push(Chain {
                origin: h.from,
                through,
                end: hops[*path.last().unwrap()].to,
                path,
            });
        }
    }
    out
}

/// `Layout → Sidebar → UserMenu → Avatar`
fn route(symbols: &[Symbol], c: &Chain) -> String {
    let mut names: Vec<&str> = Vec::new();
    if !c.through.contains(&c.origin) {
        names.push(&symbols[c.origin].name);
    }
    names.extend(c.through.iter().map(|&t| symbols[t].name.as_str()));
    names.push(&symbols[c.end].name);
    names.join(" \u{2192} ")
}

/// Put a hint on every component that only forwards a prop of a chain.
pub fn add_hints(symbols: &mut [Symbol], hops: &[Hop], chains: &[Chain]) {
    for c in chains {
        let route = route(symbols, c);
        let depth = c.through.len();
        // the attribute that forwards the prop, per component
        for &t in &c.through {
            let Some(h) = c.path.iter().map(|&k| &hops[k]).find(|h| h.from == t) else { continue };
            let text = format!(
                "`{}` is only passed on here ({route}): {depth} components forward it without using it. Consider a context, or composing the tree so the value is built where it is used.",
                h.own
            );
            let hint = DrillHint { prop: h.own.clone(), text, line: h.line, end: c.end };
            let have = &mut symbols[t].drill_hints;
            if !have.iter().any(|x| (&x.prop, x.line) == (&hint.prop, hint.line)) {
                have.push(hint);
            }
        }
    }
}
