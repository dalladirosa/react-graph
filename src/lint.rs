//! A few React rules that only need one function's syntax: hooks called out of order, effects that
//! never clean up, state derived in an effect, and list items without a `key`.
//!
//! Each rule is conservative: it only reports shapes that are wrong (or almost always a mistake)
//! on their own, so a hit is worth a look.

use crate::graph::{call_args, callee_name, defines_symbol, free_refs, is_fn, root_ident, strip_expr, text, SymKind};
use tree_sitter::Node;

#[derive(Clone, Debug)]
pub struct LintHint {
    /// "hook-order", "effect-cleanup", "derived-state", "missing-key"
    pub rule: &'static str,
    pub text: String,
    /// 1-based
    pub line: usize,
}

fn hint(rule: &'static str, n: Node, text: String) -> LintHint {
    LintHint { rule, text, line: n.start_position().row + 1 }
}

/// `useState`, `useFoo`, `use2FA` (not `use`, `user`, `used`)
pub fn is_hook_name(n: &str) -> bool {
    n.strip_prefix("use").and_then(|r| r.chars().next()).is_some_and(|c| c.is_uppercase() || c.is_ascii_digit())
}

// ------------------------------------------------------------------ hook order

#[derive(Clone, Copy, PartialEq, Debug)]
enum Place {
    Top,
    Condition,
    Loop,
    Callback,
}

struct HookWalk<'a> {
    src: &'a str,
    after_return: bool,
    out: Vec<LintHint>,
}

/// Does `n` return from the function it is in (a `return` that isn't inside a nested function)?
fn returns(n: Node) -> bool {
    if is_fn(n) || n.kind() == "function_declaration" {
        return false;
    }
    if n.kind() == "return_statement" {
        return true;
    }
    let mut c = n.walk();
    let kids: Vec<Node> = n.children(&mut c).collect();
    kids.into_iter().any(returns)
}

impl HookWalk<'_> {
    fn call(&mut self, n: Node, place: Place) {
        let Some(name) = callee_name(n, self.src).filter(|c| is_hook_name(c)) else { return };
        let why = match place {
            Place::Condition => "is called conditionally",
            Place::Loop => "is called in a loop",
            Place::Callback => "is called inside a nested function or callback",
            Place::Top if self.after_return => "is called after an early return",
            Place::Top => return,
        };
        let fix = match place {
            Place::Callback => "Hooks can only be called at the top level of a component or a custom hook.",
            _ => "Hooks must run in the same order on every render: call it before any condition, loop or early return, and put the condition inside it.",
        };
        self.out.push(hint("hook-order", n, format!("`{name}` {why}. {fix}")));
    }

    fn walk(&mut self, n: Node, place: Place) {
        // nested named functions are symbols of their own and checked on their own
        if defines_symbol(n, self.src) {
            return;
        }
        // inside something already out of order, the first reason stays
        let inside = |p: Place| if place == Place::Top { p } else { place };
        let mut c = n.walk();
        let kids: Vec<Node> = n.children(&mut c).collect();
        match n.kind() {
            "arrow_function" | "function_expression" | "function" => {
                for k in kids {
                    self.walk(k, Place::Callback);
                }
            }
            // the condition always runs; what it guards does not
            "if_statement" | "ternary_expression" => {
                let cond = n.child_by_field_name("condition").map(|c| c.id());
                for k in kids {
                    self.walk(k, if Some(k.id()) == cond { place } else { inside(Place::Condition) });
                }
            }
            "binary_expression" if matches!(n.child_by_field_name("operator").map(|o| text(o, self.src)), Some("&&" | "||" | "??")) => {
                let left = n.child_by_field_name("left").map(|l| l.id());
                for k in kids {
                    self.walk(k, if Some(k.id()) == left { place } else { inside(Place::Condition) });
                }
            }
            "for_statement" | "for_in_statement" | "while_statement" | "do_statement" => {
                for k in kids {
                    self.walk(k, inside(Place::Loop));
                }
            }
            "switch_statement" => {
                for k in kids {
                    self.walk(k, inside(Place::Condition));
                }
            }
            _ => {
                if n.kind() == "call_expression" {
                    self.call(n, place);
                }
                for k in kids {
                    self.walk(k, place);
                }
            }
        }
    }
}

/// Hooks called where React can't keep their order. `body` is the function's body (or the
/// expression of an arrow function). Components and hooks are checked for conditions, loops, nested
/// functions and early returns; any other function must not call hooks at all.
pub fn hook_rules(body: Node, kind: SymKind, name: &str, src: &str) -> Vec<LintHint> {
    let mut w = HookWalk { src, after_return: false, out: Vec::new() };
    if kind == SymKind::Function {
        // not a component or a hook: any hook call is wrong
        let mut all = Vec::new();
        fn collect<'t>(n: Node<'t>, src: &str, out: &mut Vec<Node<'t>>) {
            if defines_symbol(n, src) {
                return;
            }
            if n.kind() == "call_expression" && callee_name(n, src).is_some_and(is_hook_name) {
                out.push(n);
            }
            let mut c = n.walk();
            let kids: Vec<Node> = n.children(&mut c).collect();
            for k in kids {
                collect(k, src, out);
            }
        }
        collect(body, src, &mut all);
        return all
            .into_iter()
            .map(|n| {
                let hook = callee_name(n, src).unwrap_or("");
                hint(
                    "hook-order",
                    n,
                    format!("`{hook}` is called in `{name}`, which is neither a component nor a custom hook. Name it `use...` (or make it a component), or call the hook from one."),
                )
            })
            .collect();
    }
    if body.kind() == "statement_block" {
        let mut c = body.walk();
        let stmts: Vec<Node> = body.named_children(&mut c).collect();
        for st in stmts {
            w.walk(st, Place::Top);
            // `if (!ready) return null;` skips everything below it on some renders
            if st.kind() == "if_statement" && returns(st) {
                w.after_return = true;
            }
        }
    } else {
        w.walk(body, Place::Top);
    }
    w.out
}

// ------------------------------------------------------------------ effects

/// What a call registers that has to be undone: (what, node).
fn registration<'t>(n: Node<'t>, src: &str) -> Option<(&'static str, Node<'t>)> {
    match n.kind() {
        "call_expression" => match callee_name(n, src)? {
            // `{ once: true }` removes itself
            "addEventListener" if listens_once(n, src) => None,
            "addEventListener" => Some(("an event listener", n)),
            "setInterval" => Some(("an interval", n)),
            "subscribe" => Some(("a subscription", n)),
            "observe" if n.child_by_field_name("function").is_some_and(|f| f.kind() == "member_expression") => {
                Some(("an observer", n))
            }
            _ => None,
        },
        "new_expression" => match n.child_by_field_name("constructor").map(|c| text(c, src)) {
            Some("WebSocket") => Some(("a WebSocket", n)),
            Some("EventSource") => Some(("an EventSource", n)),
            _ => None,
        },
        _ => None,
    }
}

/// `el.addEventListener("x", fn, { once: true })`
fn listens_once(call: Node, src: &str) -> bool {
    let Some(opts) = call_args(call).get(2).map(|o| strip_expr(*o)).filter(|o| o.kind() == "object") else { return false };
    let mut c = opts.walk();
    let found = opts.named_children(&mut c).filter(|p| p.kind() == "pair").any(|p| {
        p.child_by_field_name("key").is_some_and(|k| text(k, src) == "once")
            && p.child_by_field_name("value").is_some_and(|v| text(v, src) == "true")
    });
    found
}

fn first_registration<'t>(n: Node<'t>, src: &str) -> Option<(&'static str, Node<'t>)> {
    if let Some(r) = registration(n, src) {
        return Some(r);
    }
    let mut c = n.walk();
    let kids: Vec<Node<'t>> = n.children(&mut c).collect();
    kids.into_iter().find_map(|k| first_registration(k, src))
}

/// A `return <value>` in the effect itself (not in a function it defines).
fn returns_value(n: Node) -> bool {
    if is_fn(n) || n.kind() == "function_declaration" {
        return false;
    }
    if n.kind() == "return_statement" {
        return n.named_child_count() > 0;
    }
    let mut c = n.walk();
    let kids: Vec<Node> = n.children(&mut c).collect();
    kids.into_iter().any(returns_value)
}

/// The function a node is written in.
pub fn enclosing_fn(n: Node) -> Option<Node> {
    let mut p = n.parent();
    while let Some(x) = p {
        if is_fn(x) || x.kind() == "function_declaration" {
            return Some(x);
        }
        p = x.parent();
    }
    None
}

/// Is `name` read or called anywhere in `n` outside `skip` (its own declaration `[x, setX] = ...` aside)?
fn used_outside(n: Node, name: &str, skip: Node, src: &str) -> bool {
    if n.id() == skip.id() {
        return false;
    }
    if n.kind() == "identifier" {
        return text(n, src) == name && n.parent().is_some_and(|p| p.kind() != "array_pattern");
    }
    let mut c = n.walk();
    let kids: Vec<Node> = n.children(&mut c).collect();
    kids.into_iter().any(|k| used_outside(k, name, skip, src))
}

/// Rules for the callback of `useEffect(() => { ... }, deps)`. `is_setter`: is this name a
/// `useState` / `useReducer` setter of the component, which is the function `owner`.
pub fn effect_rules(call: Node, hook: &str, is_setter: &dyn Fn(&str) -> bool, owner: Option<Node>, src: &str) -> Vec<LintHint> {
    let mut out = Vec::new();
    let args = call_args(call);
    let Some(cb) = args.first().map(|a| strip_expr(*a)).filter(|a| is_fn(*a)) else { return out };
    let Some(body) = cb.child_by_field_name("body").filter(|b| b.kind() == "statement_block") else { return out };

    // listeners, timers and subscriptions that are never undone
    if !returns_value(body) {
        if let Some((what, n)) = first_registration(body, src) {
            out.push(hint(
                "effect-cleanup",
                n,
                format!("`{hook}` sets up {what} but returns no cleanup function, so it piles up every time the effect re-runs and leaks after the component unmounts. Return a function that undoes it."),
            ));
        }
    }

    // `useEffect(() => { setX(f(a, b)) }, [a, b])`: state that is just a function of other values
    let deps = args.get(1).map(|d| strip_expr(*d)).filter(|d| d.kind() == "array");
    if let Some(deps) = deps {
        let mut c = deps.walk();
        let roots: Vec<String> = deps.named_children(&mut c).filter_map(|d| root_ident(d, src)).collect();
        let mut c = body.walk();
        let stmts: Vec<Node> = body.named_children(&mut c).filter(|s| s.kind() != "comment").collect();
        let mut derived: Option<(String, Node)> = None;
        let all_derive = !stmts.is_empty()
            && stmts.iter().all(|st| {
                if st.kind() != "expression_statement" {
                    return false;
                }
                let Some(c) = st.named_child(0).map(strip_expr).filter(|c| c.kind() == "call_expression") else { return false };
                let Some(f) = c.child_by_field_name("function").filter(|f| f.kind() == "identifier") else { return false };
                let setter = text(f, src);
                let a = call_args(c);
                // a constant or an updater function isn't derived from the deps
                let reads_deps = a.len() == 1
                    && !is_fn(strip_expr(a[0]))
                    && free_refs(a[0], src).iter().any(|r| roots.contains(r));
                if is_setter(setter) && reads_deps && derived.is_none() {
                    derived = Some((setter.to_string(), c));
                }
                is_setter(setter) && reads_deps
            });
        // state that is also set somewhere else (an input's onChange, say) is local state the
        // effect merely re-syncs, not a value computed from the deps
        let set_elsewhere = |setter: &str| owner.is_some_and(|o| used_outside(o, setter, call, src));
        if let (true, Some((setter, n))) = (all_derive, derived.filter(|(s, _)| !set_elsewhere(s))) {
            let from = if roots.is_empty() { "other values".to_string() } else { roots.iter().map(|r| format!("`{r}`")).collect::<Vec<_>>().join(", ") };
            out.push(hint(
                "derived-state",
                n,
                format!("`{setter}` is set in an effect only from {from}. Compute it while rendering (a plain const, or useMemo if it is costly) instead: the effect adds a second render and shows stale state for a frame."),
            ));
        }
    }
    out
}

// ------------------------------------------------------------------ keys

/// What a `.map` callback returns, when it is JSX: `x => <li />`, `x => { return <li />; }`.
fn returned_jsx<'t>(cb: Node<'t>, out: &mut Vec<Node<'t>>) {
    fn jsx(n: Node) -> bool {
        matches!(n.kind(), "jsx_element" | "jsx_self_closing_element" | "jsx_fragment")
    }
    fn returns<'t>(n: Node<'t>, out: &mut Vec<Node<'t>>) {
        if is_fn(n) || n.kind() == "function_declaration" {
            return;
        }
        if n.kind() == "return_statement" {
            if let Some(v) = n.named_child(0).map(strip_expr).filter(|v| jsx(*v)) {
                out.push(v);
            }
            return;
        }
        let mut c = n.walk();
        let kids: Vec<Node<'t>> = n.children(&mut c).collect();
        for k in kids {
            returns(k, out);
        }
    }
    let Some(body) = cb.child_by_field_name("body") else { return };
    if body.kind() == "statement_block" {
        returns(body, out);
    } else if jsx(strip_expr(body)) {
        out.push(strip_expr(body));
    }
}

/// `items.map(item => <li>{item}</li>)`: the element needs a `key`.
pub fn missing_key(call: Node, src: &str) -> Vec<LintHint> {
    let mut out = Vec::new();
    let is_map = call
        .child_by_field_name("function")
        .filter(|f| f.kind() == "member_expression")
        .and_then(|f| f.child_by_field_name("property"))
        .is_some_and(|p| text(p, src) == "map");
    let Some(cb) = call_args(call).first().map(|a| strip_expr(*a)).filter(|a| is_map && is_fn(*a)) else { return out };
    let mut roots = Vec::new();
    returned_jsx(cb, &mut roots);
    for el in roots {
        let open = if el.kind() == "jsx_element" { el.child_by_field_name("open_tag").unwrap_or(el) } else { el };
        // `<>...</>` is a jsx_element whose opening tag has no name
        let fragment = el.kind() == "jsx_fragment" || open.child_by_field_name("name").is_none();
        let (what, ok) = match fragment {
            true => ("a fragment", false),
            false => {
                let mut c = open.walk();
                let has = open
                    .children(&mut c)
                    .any(|a| a.kind() == "jsx_attribute" && a.named_child(0).is_some_and(|p| text(p, src) == "key"));
                // `<li {...props}>` may carry a key in the spread
                let mut c = open.walk();
                let spread = open.children(&mut c).any(|a| a.kind() == "jsx_expression" && text(a, src).starts_with("{..."));
                ("an element", has || spread)
            }
        };
        if !ok {
            let fix = if what == "a fragment" { "Use <Fragment key={...}> instead of <>...</>." } else { "Give it a stable `key`, such as an id from the item." };
            out.push(hint("missing-key", el, format!("This list item is {what} returned from .map() with no `key`. {fix}")));
        }
    }
    out
}
