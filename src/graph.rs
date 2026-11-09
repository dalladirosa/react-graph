//! Parses a React (TS/JS) folder into a graph of components, hooks and functions.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use crate::similar::{self, Shape};
use tree_sitter::{Node, Parser};
use walkdir::WalkDir;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SymKind {
    Component,
    Hook,
    Function,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EdgeKind {
    /// Component renders another component (<Child />)
    Renders,
    /// Function calls another function / hook
    Calls,
    /// Function is defined inside another function
    Defines,
    /// Function is passed as a prop/handler (onClick={fn})
    Handler,
}

impl EdgeKind {
    pub fn label(self) -> &'static str {
        match self {
            EdgeKind::Renders => "renders",
            EdgeKind::Calls => "calls",
            EdgeKind::Defines => "defines",
            EdgeKind::Handler => "handler",
        }
    }
}

/// A React wrapper around a function: `memo(...)`, `useCallback(...)`, ...
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wrap {
    Memo,
    ForwardRef,
    UseCallback,
    UseMemo,
}

impl Wrap {
    pub fn label(self) -> &'static str {
        match self {
            Wrap::Memo => "memo",
            Wrap::ForwardRef => "forwardRef",
            Wrap::UseCallback => "useCallback",
            Wrap::UseMemo => "useMemo",
        }
    }
    fn from_callee(name: &str) -> Option<Wrap> {
        match name {
            "memo" => Some(Wrap::Memo),
            "forwardRef" => Some(Wrap::ForwardRef),
            "useCallback" => Some(Wrap::UseCallback),
            "useMemo" => Some(Wrap::UseMemo),
            _ => None,
        }
    }
}

/// Where a dependency (or a value read inside a hook callback) comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    /// component prop
    Prop,
    /// custom hook argument
    Param,
    /// `useState` / `useReducer` value
    State,
    /// state setter / dispatch: stable identity
    Setter,
    /// `useContext` value
    Context,
    /// `useRef` object: stable identity
    Ref,
    /// `useMemo` / `useCallback` result
    Memoized,
    /// value returned by a custom hook
    HookValue,
    /// object / array / function created during render: new identity every render
    Unstable,
    /// other value computed during render
    Local,
    /// declared outside the component (module scope or import)
    Outer,
}

impl Origin {
    pub fn is_stable(self) -> bool {
        matches!(self, Origin::Setter | Origin::Ref)
    }
}

#[derive(Clone, Debug)]
pub struct Dep {
    /// the dependency as written, e.g. `user.id`
    pub text: String,
    /// leftmost identifier, e.g. `user`
    pub root: Option<String>,
    pub origin: Origin,
    /// extra context: hook name, literal kind, ...
    pub detail: Option<String>,
}

impl Dep {
    pub fn origin_label(&self) -> String {
        match (self.origin, &self.detail) {
            (Origin::Prop, _) => "prop".into(),
            (Origin::Param, _) => "param".into(),
            (Origin::State, _) => "state".into(),
            (Origin::Setter, _) => "setter · stable".into(),
            (Origin::Context, _) => "context".into(),
            (Origin::Ref, _) => "ref · stable".into(),
            (Origin::Memoized, Some(d)) => format!("memoized · {d}"),
            (Origin::Memoized, None) => "memoized".into(),
            (Origin::HookValue, Some(d)) => format!("from {d}"),
            (Origin::HookValue, None) => "hook value".into(),
            (Origin::Unstable, _) => "new every render".into(),
            (Origin::Local, _) => "local".into(),
            (Origin::Outer, _) => "outer scope".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Hint {
    /// true = likely bug / wasted memo; false = informational
    pub warn: bool,
    pub text: String,
}

/// A hook call that takes a dependency array.
#[derive(Clone, Debug)]
pub struct DepInfo {
    /// "useCallback", "useMemo", "useEffect", ...
    pub hook: String,
    pub line: usize,
    /// `None` = no dependency array at all
    pub deps: Option<Vec<Dep>>,
    pub hints: Vec<Hint>,
}

impl DepInfo {
    pub fn warnings(&self) -> usize {
        self.hints.iter().filter(|h| h.warn).count()
    }
}

#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub kind: SymKind,
    pub file: String,
    pub line: usize,
    /// last line of the declaration (1-based, inclusive)
    pub end_line: usize,
    pub parent: Option<usize>,
    /// wrappers, outermost first: `memo(forwardRef(...))` => [Memo, ForwardRef]
    pub wraps: Vec<Wrap>,
    /// dependencies of the symbol itself (`const f = useCallback(..., deps)`)
    pub deps: Option<DepInfo>,
    /// other dependency-taking hooks called directly in this symbol (`useEffect`, ...)
    pub dep_hooks: Vec<DepInfo>,
    /// binds a computed value, not a function: `const rows = useMemo(() => filter(...), deps)`
    pub is_value: bool,
    /// other symbols of the same kind with an identical or near-identical body, best match first
    pub similar: Vec<Similar>,
    /// byte range of the function node in its file (what `similar` fingerprints), for side-by-side compare
    pub func: Option<(usize, usize)>,
    /// structural fingerprint; only kept until similar bodies are matched
    shape: Shape,
}

/// Another symbol whose body has (nearly) the same structure: a candidate to merge or extract.
#[derive(Clone, Copy, Debug)]
pub struct Similar {
    pub other: usize,
    /// shingle similarity, 0.8..=1.0
    pub score: f32,
    /// identical structure: only names / literal values differ
    pub exact: bool,
}

impl Symbol {
    pub fn is_memoized(&self) -> bool {
        !self.wraps.is_empty() && self.wraps.iter().any(|w| *w != Wrap::ForwardRef)
    }
    /// warnings on this symbol's own deps and the hooks it calls
    pub fn warnings(&self) -> usize {
        self.deps.as_ref().map_or(0, |d| d.warnings())
            + self.dep_hooks.iter().map(|d| d.warnings()).sum::<usize>()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub kind: EdgeKind,
    /// 1-based line of the reference (the first one when `from` uses `to` several times)
    pub line: usize,
    /// symbol whose code holds `line`: `from`, except for handler edges, where it is the
    /// component that passes the prop (`<Card onAdd={handleAdd} />` is written in the parent)
    pub site: usize,
}

#[derive(Default)]
pub struct Graph {
    pub symbols: Vec<Symbol>,
    pub edges: Vec<Edge>,
    pub files: usize,
    /// relative paths (forward slashes) of every parsed source file
    pub file_list: Vec<String>,
    out: Vec<Vec<usize>>,
    inc: Vec<Vec<usize>>,
}

impl Graph {
    pub fn out_edges(&self, i: usize) -> impl Iterator<Item = &Edge> {
        self.out[i].iter().map(move |&e| &self.edges[e])
    }
    pub fn in_edges(&self, i: usize) -> impl Iterator<Item = &Edge> {
        self.inc[i].iter().map(move |&e| &self.edges[e])
    }
}

struct RawRef {
    owner: usize,
    name: String,
    kind: EdgeKind,
    file: String,
    /// 1-based line of the reference
    line: usize,
    /// For handlers: the component receiving the function as a prop.
    via: Option<String>,
    /// For handlers: the prop is named like an event handler (`onClick`, `onSave`, ...).
    event_prop: bool,
}

/// `onClick` / `onSave` (but not `one` or `online`)
fn is_event_prop(name: &str) -> bool {
    name.strip_prefix("on").and_then(|r| r.chars().next()).is_some_and(|c| c.is_ascii_uppercase())
}

/// Does this `useMemo` callback return a function literal (`useMemo(() => () => ..., deps)`)?
fn returns_fn_literal(func: Node) -> bool {
    let Some(body) = func.child_by_field_name("body") else { return false };
    if body.kind() != "statement_block" {
        return is_fn(strip_expr(body));
    }
    let mut c = body.walk();
    let returns: Vec<Node> = body.named_children(&mut c).filter(|s| s.kind() == "return_statement").collect();
    returns.into_iter().any(|r| r.named_child(0).is_some_and(|e| is_fn(strip_expr(e))))
}

struct Ctx<'a> {
    src: &'a str,
    file: String,
    symbols: &'a mut Vec<Symbol>,
    refs: &'a mut Vec<RawRef>,
    /// `const { addItem } = useCart()` => addItem -> useCart
    aliases: &'a mut HashMap<(String, String), String>,
    /// `export default memo(Card)` => (file, "Card")
    memo_marks: &'a mut Vec<(String, String)>,
    /// `const FastCard = memo(Card)` => FastCard -> Card
    comp_alias: &'a mut HashMap<String, String>,
    /// bindings of the enclosing components / hooks, innermost last
    scopes: Vec<Scope>,
}

/// Names declared directly in a component / hook body, with their origin.
#[derive(Default)]
struct Scope {
    bindings: HashMap<String, (Origin, Option<String>)>,
}

fn lookup<'s>(scopes: &'s [Scope], name: &str) -> Option<&'s (Origin, Option<String>)> {
    scopes.iter().rev().find_map(|s| s.bindings.get(name))
}

/// `useMemo` / `React.useMemo` => "useMemo"
fn callee_name<'a>(call: Node, src: &'a str) -> Option<&'a str> {
    let f = call.child_by_field_name("function")?;
    match f.kind() {
        "identifier" => Some(text(f, src)),
        "member_expression" => f.child_by_field_name("property").map(|p| text(p, src)),
        _ => None,
    }
}

fn call_args(call: Node) -> Vec<Node> {
    match call.child_by_field_name("arguments") {
        Some(a) => {
            let mut c = a.walk();
            a.named_children(&mut c).filter(|n| n.kind() != "comment").collect()
        }
        None => Vec::new(),
    }
}

fn is_fn(n: Node) -> bool {
    matches!(n.kind(), "arrow_function" | "function_expression" | "function")
}

/// `x as T`, `x!`, `(x)`, `x satisfies T` => x
fn strip_expr(mut n: Node) -> Node {
    while matches!(
        n.kind(),
        "parenthesized_expression" | "as_expression" | "non_null_expression" | "satisfies_expression"
    ) {
        match n.named_child(0) {
            Some(c) => n = c,
            None => break,
        }
    }
    n
}

enum Unwrapped<'t> {
    /// `memo(function X() {...})`: the inner function, plus the useCallback/useMemo call if any
    Func { wraps: Vec<Wrap>, func: Node<'t>, hook_call: Option<Node<'t>> },
    /// `memo(Card)`
    Ident { target: String },
}

fn unwrap_wrappers<'t>(value: Node<'t>, src: &str) -> Option<Unwrapped<'t>> {
    let mut wraps = Vec::new();
    let mut hook_call = None;
    let mut n = strip_expr(value);
    loop {
        if n.kind() != "call_expression" {
            return None;
        }
        let w = Wrap::from_callee(callee_name(n, src)?)?;
        wraps.push(w);
        if matches!(w, Wrap::UseCallback | Wrap::UseMemo) {
            hook_call = Some(n);
        }
        let first = strip_expr(*call_args(n).first()?);
        if is_fn(first) {
            return Some(Unwrapped::Func { wraps, func: first, hook_call });
        }
        if first.kind() == "identifier" && wraps.iter().all(|w| matches!(w, Wrap::Memo | Wrap::ForwardRef)) {
            return Some(Unwrapped::Ident { target: text(first, src).to_string() });
        }
        n = first;
    }
}

/// Every name bound by a pattern (`{ a, b: c, ...rest }`, `[x, y = 1]`, `id`).
fn pattern_names(n: Node, src: &str, out: &mut Vec<String>) {
    match n.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => out.push(text(n, src).to_string()),
        "pair_pattern" => {
            if let Some(v) = n.child_by_field_name("value") {
                pattern_names(v, src, out);
            }
        }
        "object_assignment_pattern" | "assignment_pattern" => {
            if let Some(l) = n.child_by_field_name("left") {
                pattern_names(l, src, out);
            }
        }
        "required_parameter" | "optional_parameter" => {
            if let Some(p) = n.child_by_field_name("pattern") {
                pattern_names(p, src, out);
            }
        }
        "object_pattern" | "array_pattern" | "rest_pattern" | "formal_parameters" => {
            let mut c = n.walk();
            for ch in n.named_children(&mut c) {
                pattern_names(ch, src, out);
            }
        }
        _ => {}
    }
}

const NEW_ARRAY_METHODS: &[&str] = &[
    "map", "filter", "slice", "concat", "flatMap", "flat", "toSorted", "toReversed", "toSpliced",
    "keys", "values", "entries", "assign", "from", "parse", "structuredClone",
];

/// Classify `const <name> = <value>` declared directly in a component / hook body.
fn bind_declarator(name: Node, value: Option<Node>, src: &str, scope: &mut Scope) {
    let mut names = Vec::new();
    pattern_names(name, src, &mut names);
    let set_all = |scope: &mut Scope, names: &[String], o: Origin, d: Option<String>| {
        for n in names {
            scope.bindings.insert(n.clone(), (o, d.clone()));
        }
    };
    let Some(value) = value.map(strip_expr) else {
        return set_all(scope, &names, Origin::Local, None);
    };
    let value = if value.kind() == "await_expression" { value.named_child(0).unwrap_or(value) } else { value };
    match value.kind() {
        "call_expression" => {
            let callee = callee_name(value, src).unwrap_or("");
            match callee {
                "useState" | "useReducer" if name.kind() == "array_pattern" => {
                    let mut c = name.walk();
                    let elems: Vec<Node> = name.named_children(&mut c).collect();
                    for (k, e) in elems.into_iter().enumerate() {
                        let mut one = Vec::new();
                        pattern_names(e, src, &mut one);
                        let o = if k == 0 { Origin::State } else { Origin::Setter };
                        set_all(scope, &one, o, Some(callee.to_string()));
                    }
                }
                "useState" | "useReducer" => set_all(scope, &names, Origin::State, Some(callee.to_string())),
                "useContext" => {
                    let ctx = call_args(value).first().map(|a| text(*a, src).to_string());
                    set_all(scope, &names, Origin::Context, ctx)
                }
                "useRef" => set_all(scope, &names, Origin::Ref, None),
                "useMemo" | "useCallback" => set_all(scope, &names, Origin::Memoized, Some(callee.to_string())),
                h if kind_for(h) == SymKind::Hook => set_all(scope, &names, Origin::HookValue, Some(h.to_string())),
                m if NEW_ARRAY_METHODS.contains(&m) => {
                    let what = if matches!(m, "assign" | "parse" | "structuredClone") { "object" } else { "array" };
                    set_all(scope, &names, Origin::Unstable, Some(what.into()))
                }
                _ => set_all(scope, &names, Origin::Local, None),
            }
        }
        "object" => set_all(scope, &names, Origin::Unstable, Some("object".into())),
        "array" => set_all(scope, &names, Origin::Unstable, Some("array".into())),
        "arrow_function" | "function_expression" | "function" => {
            set_all(scope, &names, Origin::Unstable, Some("function".into()))
        }
        "jsx_element" | "jsx_self_closing_element" | "jsx_fragment" => {
            set_all(scope, &names, Origin::Unstable, Some("JSX element".into()))
        }
        "new_expression" => set_all(scope, &names, Origin::Unstable, Some("instance".into())),
        _ => set_all(scope, &names, Origin::Local, None),
    }
}

/// Bindings visible inside the body of component / hook function `func`.
fn build_scope(func: Node, kind: SymKind, src: &str) -> Scope {
    let mut scope = Scope::default();
    let param_origin = if kind == SymKind::Component { Origin::Prop } else { Origin::Param };
    let mut params = Vec::new();
    if let Some(p) = func.child_by_field_name("parameter") {
        pattern_names(p, src, &mut params);
    } else if let Some(ps) = func.child_by_field_name("parameters") {
        let mut c = ps.walk();
        let list: Vec<Node> = ps.named_children(&mut c).collect();
        // a component's props are its first parameter; the second is a forwarded ref
        for (k, p) in list.into_iter().enumerate() {
            let mut one = Vec::new();
            pattern_names(p, src, &mut one);
            let o = if kind == SymKind::Component && k > 0 { Origin::Ref } else { param_origin };
            for n in one {
                scope.bindings.insert(n, (o, None));
            }
        }
    }
    for n in params {
        scope.bindings.insert(n, (param_origin, None));
    }
    let Some(body) = func.child_by_field_name("body") else { return scope };
    if body.kind() != "statement_block" {
        return scope;
    }
    let mut c = body.walk();
    for st in body.named_children(&mut c) {
        match st.kind() {
            "lexical_declaration" | "variable_declaration" => {
                let mut c2 = st.walk();
                for d in st.named_children(&mut c2).filter(|d| d.kind() == "variable_declarator") {
                    if let Some(name) = d.child_by_field_name("name") {
                        bind_declarator(name, d.child_by_field_name("value"), src, &mut scope);
                    }
                }
            }
            "function_declaration" => {
                if let Some(n) = st.child_by_field_name("name") {
                    scope.bindings.insert(text(n, src).to_string(), (Origin::Unstable, Some("function".into())));
                }
            }
            _ => {}
        }
    }
    scope
}

/// Identifiers read inside `n` that are not declared inside it (in order, deduplicated).
fn free_refs(n: Node, src: &str) -> Vec<String> {
    fn walk(n: Node, src: &str, refs: &mut Vec<String>, declared: &mut Vec<String>) {
        let mut skip: Option<usize> = None;
        match n.kind() {
            "identifier" | "shorthand_property_identifier" => {
                refs.push(text(n, src).to_string());
                return;
            }
            "type_annotation" | "type_arguments" | "type_parameters" | "jsx_closing_element" => return,
            "variable_declarator" => {
                if let Some(nm) = n.child_by_field_name("name") {
                    pattern_names(nm, src, declared);
                }
                if let Some(v) = n.child_by_field_name("value") {
                    walk(v, src, refs, declared);
                }
                return;
            }
            "required_parameter" | "optional_parameter" => {
                pattern_names(n, src, declared);
                return;
            }
            "arrow_function" => {
                if let Some(p) = n.child_by_field_name("parameter") {
                    pattern_names(p, src, declared);
                    skip = Some(p.id());
                }
            }
            "function_declaration" | "function_expression" | "class_declaration" => {
                if let Some(nm) = n.child_by_field_name("name") {
                    declared.push(text(nm, src).to_string());
                    skip = Some(nm.id());
                }
            }
            "catch_clause" => {
                if let Some(p) = n.child_by_field_name("parameter") {
                    pattern_names(p, src, declared);
                    skip = Some(p.id());
                }
            }
            _ => {}
        }
        let mut c = n.walk();
        let kids: Vec<Node> = n.children(&mut c).collect();
        for k in kids {
            if Some(k.id()) != skip {
                walk(k, src, refs, declared);
            }
        }
    }
    let (mut refs, mut declared) = (Vec::new(), Vec::new());
    walk(n, src, &mut refs, &mut declared);
    let mut seen = HashSet::new();
    refs.into_iter()
        .filter(|r| !declared.contains(r) && seen.insert(r.clone()))
        .collect()
}

/// `user?.profile.id` => "user"
fn root_ident(mut n: Node, src: &str) -> Option<String> {
    loop {
        n = strip_expr(n);
        match n.kind() {
            "identifier" => return Some(text(n, src).to_string()),
            "member_expression" | "subscript_expression" => n = n.child_by_field_name("object")?,
            _ => return None,
        }
    }
}

/// useEffect / useLayoutEffect / useInsertionEffect (as opposed to useCallback / useMemo)
pub fn is_effect(hook: &str) -> bool {
    !matches!(hook, "useCallback" | "useMemo")
}

/// What a needless re-run of this hook costs, for hint messages.
fn rerun_phrase(hook: &str) -> &'static str {
    match hook {
        "useCallback" => "useCallback returns a new function on every render",
        "useMemo" => "useMemo recomputes on every render",
        _ => "the effect runs after every render",
    }
}

/// Analyse the deps of a `useCallback` / `useMemo` / `useEffect`-like call.
fn analyze_deps(call: Node, hook: &str, self_name: Option<&str>, src: &str, scopes: &[Scope]) -> DepInfo {
    let args = call_args(call);
    let cb = args.first().map(|a| strip_expr(*a));
    let deps_node = args.get(1).map(|a| strip_expr(*a));
    let in_component = !scopes.is_empty();
    let mut hints = Vec::new();
    let mut checkable = true;

    let deps: Option<Vec<Dep>> = match deps_node {
        None => None,
        Some(arr) if arr.kind() == "array" => {
            let mut c = arr.walk();
            let elems: Vec<Node> = arr.named_children(&mut c).filter(|n| n.kind() != "comment").collect();
            Some(
                elems
                    .into_iter()
                    .map(|e| {
                        let root = root_ident(e, src);
                        let (origin, detail) = match root.as_deref().and_then(|r| lookup(scopes, r)) {
                            Some((o, d)) => (*o, d.clone()),
                            None if root.is_some() => (Origin::Outer, None),
                            None => (Origin::Local, None),
                        };
                        Dep { text: text(e, src).to_string(), root, origin, detail }
                    })
                    .collect(),
            )
        }
        Some(other) => {
            checkable = false;
            hints.push(Hint {
                warn: false,
                text: format!("The dependency list is `{}`, not an array literal, so it can't be checked.", text(other, src)),
            });
            Some(Vec::new())
        }
    };

    match &deps {
        None => {
            let effect = is_effect(hook);
            hints.push(Hint {
                warn: !effect,
                text: if effect {
                    "No dependency array: the effect runs after every render.".into()
                } else {
                    format!("No dependency array: {}, so it has no effect.", rerun_phrase(hook))
                },
            });
        }
        Some(list) if checkable && in_component => {
            // values read inside the callback but not listed
            if let Some(cb) = cb.filter(|c| is_fn(*c)) {
                let roots: HashSet<&str> = list.iter().filter_map(|d| d.root.as_deref()).collect();
                for r in free_refs(cb, src) {
                    if Some(r.as_str()) == self_name || roots.contains(r.as_str()) {
                        continue;
                    }
                    let Some((o, d)) = lookup(scopes, &r) else { continue };
                    if o.is_stable() {
                        continue;
                    }
                    let what = Dep { text: r.clone(), root: None, origin: *o, detail: d.clone() }.origin_label();
                    hints.push(Hint {
                        warn: true,
                        text: format!(
                            "Missing dependency `{r}` ({what}): the {} can read a stale value.",
                            if is_effect(hook) { "effect" } else { "callback" }
                        ),
                    });
                }
            }
            for d in list {
                match d.origin {
                    Origin::Unstable => hints.push(unstable_hint(d, hook)),
                    Origin::Outer => hints.push(Hint {
                        warn: false,
                        text: format!(
                            "`{}` is declared outside the component and never triggers a re-render; it doesn't need to be listed.",
                            d.text
                        ),
                    }),
                    _ => {}
                }
            }
        }
        _ => {}
    }

    DepInfo {
        hook: hook.to_string(),
        line: call.start_position().row + 1,
        deps,
        hints,
    }
}

fn unstable_hint(d: &Dep, hook: &str) -> Hint {
    let what = d.detail.as_deref().unwrap_or("value");
    let fix = if what == "function" { "useCallback" } else { "useMemo" };
    Hint {
        warn: true,
        text: format!(
            "`{}` is a new {what} on every render, so {}. Wrap it in {fix} or move it inside.",
            d.text,
            rerun_phrase(hook)
        ),
    }
}

const DEP_HOOKS: &[&str] = &[
    "useEffect", "useLayoutEffect", "useInsertionEffect", "useCallback", "useMemo",
];

fn kind_for(name: &str) -> SymKind {
    if name.starts_with("use")
        && name.chars().nth(3).map(|c| c.is_uppercase()).unwrap_or(false)
    {
        SymKind::Hook
    } else if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
        SymKind::Component
    } else {
        SymKind::Function
    }
}

fn text<'a>(n: Node, src: &'a str) -> &'a str {
    n.utf8_text(src.as_bytes()).unwrap_or("")
}

fn visit(node: Node, owner: Option<usize>, cx: &mut Ctx) {
    match node.kind() {
        "function_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                let id = new_symbol(text(name, cx.src), node, owner, cx);
                if let Some(body) = node.child_by_field_name("body") {
                    enter_fn(id, node, body, cx);
                }
                return;
            }
        }
        "variable_declarator" => {
            let name = node.child_by_field_name("name");
            let value = node.child_by_field_name("value");
            if let (Some(name), Some(value)) = (name, value) {
                if name.kind() == "identifier" && is_fn(strip_expr(value)) {
                    let id = new_symbol(text(name, cx.src), node, owner, cx);
                    enter_fn(id, strip_expr(value), strip_expr(value), cx);
                    return;
                }
                // const Card = memo(function Card() {...}) / const f = useCallback(() => ..., deps)
                if name.kind() == "identifier" {
                    let sym_name = text(name, cx.src);
                    match unwrap_wrappers(value, cx.src) {
                        Some(Unwrapped::Func { wraps, func, hook_call }) => {
                            let id = new_symbol(sym_name, node, owner, cx);
                            if let Some(call) = hook_call {
                                let hook = callee_name(call, cx.src).unwrap_or("useCallback");
                                let info = analyze_deps(call, hook, Some(sym_name), cx.src, &cx.scopes);
                                cx.symbols[id].deps = Some(info);
                                cx.symbols[id].is_value = hook == "useMemo" && !returns_fn_literal(func);
                            }
                            cx.symbols[id].wraps = wraps;
                            enter_fn(id, func, func, cx);
                            return;
                        }
                        Some(Unwrapped::Ident { target }) => {
                            // const FastCard = memo(Card)
                            cx.memo_marks.push((cx.file.clone(), target.clone()));
                            if sym_name != target {
                                cx.comp_alias.insert(sym_name.to_string(), target);
                            }
                            return;
                        }
                        None => {}
                    }
                }
                // const { a, b } = useSomething();
                if name.kind() == "object_pattern" && value.kind() == "call_expression" {
                    if let Some(f) = value.child_by_field_name("function") {
                        if f.kind() == "identifier" {
                            let hook = text(f, cx.src).to_string();
                            let mut c = name.walk();
                            for ch in name.children(&mut c) {
                                let n = match ch.kind() {
                                    "shorthand_property_identifier_pattern" => Some(ch),
                                    "pair_pattern" => ch.child_by_field_name("value"),
                                    _ => None,
                                };
                                if let Some(n) = n {
                                    cx.aliases.insert(
                                        (cx.file.clone(), text(n, cx.src).to_string()),
                                        hook.clone(),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        "jsx_opening_element" | "jsx_self_closing_element" => {
            if let (Some(owner), Some(name)) = (owner, node.child_by_field_name("name")) {
                let n = text(name, cx.src);
                if n.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                    cx.refs.push(RawRef {
                        owner,
                        name: n.to_string(),
                        kind: EdgeKind::Renders,
                        file: cx.file.clone(),
                        line: node.start_position().row + 1,
                        via: None,
                        event_prop: false,
                    });
                    // onClick={handleAdd} => <Name> receives handleAdd
                    let mut c = node.walk();
                    let attrs: Vec<Node> = node.children(&mut c).collect();
                    for a in attrs.into_iter().filter(|a| a.kind() == "jsx_attribute") {
                        let event_prop = a
                            .named_child(0)
                            .is_some_and(|p| p.kind() == "property_identifier" && is_event_prop(text(p, cx.src)));
                        let mut c2 = a.walk();
                        let kids: Vec<Node> = a.children(&mut c2).collect();
                        for e in kids.into_iter().filter(|k| k.kind() == "jsx_expression") {
                            if e.named_child_count() == 1 {
                                let id = e.named_child(0).unwrap();
                                if id.kind() == "identifier" {
                                    cx.refs.push(RawRef {
                                        owner,
                                        name: text(id, cx.src).to_string(),
                                        kind: EdgeKind::Handler,
                                        file: cx.file.clone(),
                                        line: a.start_position().row + 1,
                                        via: Some(n.to_string()),
                                        event_prop,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        "call_expression" => {
            let callee = callee_name(node, cx.src);
            // export default memo(Card)
            if callee == Some("memo") {
                if let Some(a) = call_args(node).first().map(|a| strip_expr(*a)) {
                    if a.kind() == "identifier" {
                        cx.memo_marks.push((cx.file.clone(), text(a, cx.src).to_string()));
                    }
                }
            }
            // useEffect(() => ..., deps) and other anonymous dependency hooks
            if let (Some(owner), Some(h)) = (owner, callee) {
                if DEP_HOOKS.contains(&h) {
                    let info = analyze_deps(node, h, None, cx.src, &cx.scopes);
                    cx.symbols[owner].dep_hooks.push(info);
                }
            }
            if let (Some(owner), Some(f)) = (owner, node.child_by_field_name("function")) {
                if f.kind() == "identifier" {
                    cx.refs.push(RawRef {
                        owner,
                        name: text(f, cx.src).to_string(),
                        kind: EdgeKind::Calls,
                        file: cx.file.clone(),
                        line: node.start_position().row + 1,
                        via: None,
                        event_prop: false,
                    });
                }
            }
        }
        _ => {}
    }
    let mut c = node.walk();
    let children: Vec<Node> = node.children(&mut c).collect();
    for ch in children {
        visit(ch, owner, cx);
    }
}

fn new_symbol(name: &str, node: Node, parent: Option<usize>, cx: &mut Ctx) -> usize {
    cx.symbols.push(Symbol {
        name: name.to_string(),
        kind: kind_for(name),
        file: cx.file.clone(),
        line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        parent,
        wraps: Vec::new(),
        deps: None,
        dep_hooks: Vec::new(),
        is_value: false,
        similar: Vec::new(),
        func: None,
        shape: Shape::default(),
    });
    cx.symbols.len() - 1
}

/// Visit `body` as symbol `id`; components and hooks open a new binding scope (`func` is the function node).
fn enter_fn(id: usize, func: Node, body: Node, cx: &mut Ctx) {
    let roots: Vec<Node> = FN_ROOTS
        .iter()
        .filter_map(|f| func.child_by_field_name(f))
        .collect();
    let src = cx.src;
    cx.symbols[id].shape = similar::shape(&roots, |n| defines_symbol(n, src));
    cx.symbols[id].func = Some((func.start_byte(), func.end_byte()));
    let kind = cx.symbols[id].kind;
    let scoped = matches!(kind, SymKind::Component | SymKind::Hook);
    if scoped {
        cx.scopes.push(build_scope(func, kind, cx.src));
    }
    visit(body, Some(id), cx);
    if scoped {
        cx.scopes.pop();
    }
}

/// The subtrees of a function node that `similar::shape` fingerprints.
pub(crate) const FN_ROOTS: [&str; 3] = ["parameters", "parameter", "body"];

/// Does `n` declare a function that `visit` turns into its own symbol?
pub(crate) fn defines_symbol(n: Node, src: &str) -> bool {
    match n.kind() {
        "function_declaration" => n.child_by_field_name("name").is_some(),
        "variable_declarator" => {
            let (Some(name), Some(value)) = (n.child_by_field_name("name"), n.child_by_field_name("value")) else {
                return false;
            };
            name.kind() == "identifier"
                && (is_fn(strip_expr(value)) || matches!(unwrap_wrappers(value, src), Some(Unwrapped::Func { .. })))
        }
        _ => false,
    }
}

/// Fill `Symbol::similar`: pairs of same-kind symbols whose bodies are (nearly) structurally identical.
fn find_similar(symbols: &mut [Symbol]) {
    let mut cand: Vec<usize> = (0..symbols.len()).filter(|&i| symbols[i].shape.nodes >= similar::MIN_NODES).collect();
    cand.sort_by_key(|&i| symbols[i].shape.len());
    let mut found: Vec<(usize, usize, f32, bool)> = Vec::new();
    for (pos, &a) in cand.iter().enumerate() {
        let la = symbols[a].shape.len() as f32;
        for &b in &cand[pos + 1..] {
            // sorted by size: once b is too much bigger, no later b can reach the threshold
            if la < similar::MIN_SCORE * symbols[b].shape.len() as f32 {
                break;
            }
            if symbols[a].kind != symbols[b].kind
                || is_ancestor_or_self(symbols, a, b)
                || is_ancestor_or_self(symbols, b, a)
            {
                continue;
            }
            if let Some((score, exact)) = similar::compare(&symbols[a].shape, &symbols[b].shape) {
                found.push((a, b, score, exact));
            }
        }
    }
    for (a, b, score, exact) in found {
        symbols[a].similar.push(Similar { other: b, score, exact });
        symbols[b].similar.push(Similar { other: a, score, exact });
    }
    for s in symbols.iter_mut() {
        s.similar.sort_by(|x, y| y.score.total_cmp(&x.score).then(x.other.cmp(&y.other)));
        s.shape = Shape::default();
    }
}

fn is_ancestor_or_self(symbols: &[Symbol], anc: usize, mut of: usize) -> bool {
    loop {
        if anc == of {
            return true;
        }
        match symbols[of].parent {
            Some(p) => of = p,
            None => return false,
        }
    }
}

/// What a local name was imported as.
#[derive(Clone, Debug)]
enum Imported {
    Named(String),
    Default,
    Namespace,
}

#[derive(Clone, Debug)]
struct Import {
    spec: String,
    name: Imported,
}

#[derive(Clone, Debug)]
enum ReExport {
    /// `export * from "spec"`
    Star { spec: String },
    /// `export { imported as exported } from "spec"`
    Named { spec: String, imported: String, exported: String },
}

/// Module-level facts used to find the file that defines an imported name.
#[derive(Default, Debug)]
struct Module {
    /// local name -> where it was imported from (also `const X = lazy(() => import("..."))`)
    imports: HashMap<String, Import>,
    /// exported name -> local name (`export { a as b }`, `export default X`)
    exports: HashMap<String, String>,
    reexports: Vec<ReExport>,
    /// `const FastCard = memo(Card)` => FastCard -> Card
    aliases: HashMap<String, String>,
}

/// `"./Button"` => `./Button`; identifiers pass through.
fn string_value(n: Node, src: &str) -> String {
    let t = text(n, src);
    if n.kind() == "string" {
        t.trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string()
    } else {
        t.to_string()
    }
}

/// `memo(X)`, `withAuth(X, opts)`, `memo(forwardRef(X))` => X
fn first_ident_arg(n: Node, src: &str) -> Option<String> {
    let mut n = strip_expr(n);
    for _ in 0..8 {
        match n.kind() {
            "identifier" => return Some(text(n, src).to_string()),
            "call_expression" => n = strip_expr(*call_args(n).first()?),
            _ => return None,
        }
    }
    None
}

/// `lazy(() => import("./Page"))` / `React.lazy(...)` => "./Page"
fn lazy_import_spec(value: Node, src: &str) -> Option<String> {
    fn find(n: Node, src: &str) -> Option<String> {
        if n.kind() == "call_expression" && n.child_by_field_name("function").is_some_and(|f| f.kind() == "import") {
            return call_args(n).first().filter(|a| a.kind() == "string").map(|a| string_value(*a, src));
        }
        let mut c = n.walk();
        let kids: Vec<Node> = n.named_children(&mut c).collect();
        kids.into_iter().find_map(|k| find(k, src))
    }
    let value = strip_expr(value);
    if value.kind() != "call_expression" || callee_name(value, src) != Some("lazy") {
        return None;
    }
    find(*call_args(value).first()?, src)
}

/// Top-level `const` declarations: lazy imports and memo aliases.
fn collect_declarators(decl: Node, src: &str, m: &mut Module) {
    let mut c = decl.walk();
    for d in decl.named_children(&mut c).filter(|d| d.kind() == "variable_declarator") {
        let (Some(name), Some(value)) = (d.child_by_field_name("name"), d.child_by_field_name("value")) else {
            continue;
        };
        if name.kind() != "identifier" {
            continue;
        }
        let name = text(name, src).to_string();
        if let Some(spec) = lazy_import_spec(value, src) {
            m.imports.insert(name, Import { spec, name: Imported::Default });
        } else if let Some(Unwrapped::Ident { target }) = unwrap_wrappers(value, src) {
            if target != name {
                m.aliases.insert(name, target);
            }
        }
    }
}

fn collect_import(st: Node, src: &str, m: &mut Module) {
    let Some(spec) = st.child_by_field_name("source").map(|s| string_value(s, src)) else { return };
    let mut c = st.walk();
    let clauses: Vec<Node> = st.named_children(&mut c).filter(|n| n.kind() == "import_clause").collect();
    for clause in clauses {
        let mut c2 = clause.walk();
        let parts: Vec<Node> = clause.named_children(&mut c2).collect();
        for part in parts {
            match part.kind() {
                "identifier" => {
                    m.imports.insert(text(part, src).into(), Import { spec: spec.clone(), name: Imported::Default });
                }
                "namespace_import" => {
                    if let Some(id) = part.named_child(0) {
                        m.imports.insert(text(id, src).into(), Import { spec: spec.clone(), name: Imported::Namespace });
                    }
                }
                "named_imports" => {
                    let mut c3 = part.walk();
                    for s in part.named_children(&mut c3).filter(|s| s.kind() == "import_specifier") {
                        let Some(name) = s.child_by_field_name("name").map(|n| string_value(n, src)) else { continue };
                        let local = s.child_by_field_name("alias").map(|a| text(a, src).to_string()).unwrap_or_else(|| name.clone());
                        let imported = if name == "default" { Imported::Default } else { Imported::Named(name) };
                        m.imports.insert(local, Import { spec: spec.clone(), name: imported });
                    }
                }
                _ => {}
            }
        }
    }
}

fn collect_export(st: Node, src: &str, m: &mut Module) {
    let source = st.child_by_field_name("source").map(|s| string_value(s, src));
    let mut c = st.walk();
    let kids: Vec<Node> = st.children(&mut c).collect();
    let is_default = kids.iter().any(|k| k.kind() == "default");

    // export const X = ... / export default function X() {}
    if let Some(decl) = st.child_by_field_name("declaration") {
        match decl.kind() {
            "lexical_declaration" | "variable_declaration" => collect_declarators(decl, src, m),
            _ if is_default => {
                if let Some(n) = decl.child_by_field_name("name") {
                    m.exports.insert("default".into(), text(n, src).into());
                }
            }
            _ => {}
        }
        return;
    }
    // export default X / memo(X) / withAuth(X, ...)
    if let Some(value) = st.child_by_field_name("value") {
        if is_default {
            if let Some(n) = first_ident_arg(value, src) {
                m.exports.insert("default".into(), n);
            }
        }
        return;
    }
    match (kids.iter().find(|k| k.kind() == "export_clause"), source) {
        (Some(clause), source) => {
            let mut c2 = clause.walk();
            for s in clause.named_children(&mut c2).filter(|s| s.kind() == "export_specifier") {
                let Some(name) = s.child_by_field_name("name").map(|n| string_value(n, src)) else { continue };
                let exported = s.child_by_field_name("alias").map(|a| string_value(a, src)).unwrap_or_else(|| name.clone());
                match &source {
                    Some(spec) => m.reexports.push(ReExport::Named { spec: spec.clone(), imported: name, exported }),
                    None => {
                        m.exports.insert(exported, name);
                    }
                }
            }
        }
        (None, Some(spec)) if !kids.iter().any(|k| k.kind() == "namespace_export") => {
            m.reexports.push(ReExport::Star { spec })
        }
        _ => {}
    }
}

fn collect_module(root: Node, src: &str) -> Module {
    let mut m = Module::default();
    let mut c = root.walk();
    for st in root.named_children(&mut c) {
        match st.kind() {
            "import_statement" => collect_import(st, src, &mut m),
            "export_statement" => collect_export(st, src, &mut m),
            "lexical_declaration" | "variable_declaration" => collect_declarators(st, src, &mut m),
            _ => {}
        }
    }
    m
}

const RESOLVE_SUFFIXES: &[&str] =
    &["", ".tsx", ".ts", ".jsx", ".js", "/index.tsx", "/index.ts", "/index.jsx", "/index.js"];

/// Follows relative imports, re-exports and barrels to the symbol a name refers to.
struct Resolver<'a> {
    modules: &'a HashMap<String, Module>,
    files: HashSet<&'a str>,
    /// file -> name -> first top-level symbol with that name
    top: HashMap<&'a str, HashMap<&'a str, usize>>,
}

impl<'a> Resolver<'a> {
    fn new(symbols: &'a [Symbol], modules: &'a HashMap<String, Module>, file_list: &'a [String]) -> Self {
        let mut top: HashMap<&str, HashMap<&str, usize>> = HashMap::new();
        for (i, s) in symbols.iter().enumerate().filter(|(_, s)| s.parent.is_none()) {
            top.entry(s.file.as_str()).or_default().entry(s.name.as_str()).or_insert(i);
        }
        Resolver { modules, files: file_list.iter().map(|f| f.as_str()).collect(), top }
    }

    /// `"../ui"` imported from `src/a/B.tsx` => `src/ui/index.ts` (if it was parsed)
    fn resolve_spec(&self, from: &str, spec: &str) -> Option<&'a str> {
        if !spec.starts_with('.') {
            return None; // package or path alias
        }
        let mut parts: Vec<&str> = from.split('/').collect();
        parts.pop();
        for seg in spec.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    parts.pop()?;
                }
                s => parts.push(s),
            }
        }
        let base = parts.join("/");
        RESOLVE_SUFFIXES.iter().find_map(|sfx| self.files.get(format!("{base}{sfx}").as_str()).copied())
    }

    fn find_export(&self, file: &str, name: &str, depth: usize) -> Option<usize> {
        if depth > 12 {
            return None;
        }
        let m = self.modules.get(file)?;
        let local = match m.exports.get(name) {
            Some(l) => Some(l.as_str()),
            None if name == "default" => None,
            None => Some(name),
        };
        if let Some(local) = local {
            let target = m.aliases.get(local).map(|s| s.as_str()).unwrap_or(local);
            if let Some(&i) = self.top.get(file).and_then(|t| t.get(target)) {
                return Some(i);
            }
            // import { X } from "./x"; export { X };
            if let Some(imp) = m.imports.get(target) {
                return self.resolve_import(file, imp, depth + 1);
            }
        }
        m.reexports.iter().find_map(|re| match re {
            ReExport::Star { spec } if name != "default" => {
                self.resolve_spec(file, spec).and_then(|f| self.find_export(f, name, depth + 1))
            }
            ReExport::Named { spec, imported, exported } if exported == name => {
                self.resolve_spec(file, spec).and_then(|f| self.find_export(f, imported, depth + 1))
            }
            _ => None,
        })
    }

    fn resolve_import(&self, file: &str, imp: &Import, depth: usize) -> Option<usize> {
        let f = self.resolve_spec(file, &imp.spec)?;
        match &imp.name {
            Imported::Named(n) => self.find_export(f, n, depth),
            Imported::Default => self.find_export(f, "default", depth),
            Imported::Namespace => None,
        }
    }

    /// The symbol `name`, as used in `file`, refers to through that file's imports.
    fn resolve_local(&self, file: &str, name: &str) -> Option<usize> {
        let imp = self.modules.get(file)?.imports.get(name)?;
        self.resolve_import(file, imp, 0)
    }
}

struct Part {
    symbols: Vec<Symbol>,
    refs: Vec<RawRef>,
    aliases: HashMap<(String, String), String>,
    memo_marks: Vec<(String, String)>,
    comp_alias: HashMap<String, String>,
    modules: HashMap<String, Module>,
    files: usize,
    file_list: Vec<String>,
}

fn parse_chunk(root: &Path, paths: &[PathBuf]) -> Part {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
        .expect("load tsx grammar");
    let mut part = Part {
        symbols: Vec::new(),
        refs: Vec::new(),
        aliases: HashMap::new(),
        memo_marks: Vec::new(),
        comp_alias: HashMap::new(),
        modules: HashMap::new(),
        files: 0,
        file_list: Vec::new(),
    };
    for p in paths {
        let Ok(src) = std::fs::read_to_string(p) else { continue };
        let Some(tree) = parser.parse(&src, None) else { continue };
        part.files += 1;
        let rel = p
            .strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/");
        part.file_list.push(rel.clone());
        part.modules.insert(rel.clone(), collect_module(tree.root_node(), &src));
        let mut cx = Ctx {
            src: &src,
            file: rel,
            symbols: &mut part.symbols,
            refs: &mut part.refs,
            aliases: &mut part.aliases,
            memo_marks: &mut part.memo_marks,
            comp_alias: &mut part.comp_alias,
            scopes: Vec::new(),
        };
        visit(tree.root_node(), None, &mut cx);
    }
    part
}

/// `export default memo(Card)`: flag the top-level `Card` (same file preferred) as memoized.
fn apply_memo_marks(symbols: &mut [Symbol], marks: &[(String, String)]) {
    for (file, name) in marks {
        let top = |s: &Symbol| s.parent.is_none() && &s.name == name;
        let hit = symbols
            .iter()
            .position(|s| top(s) && &s.file == file)
            .or_else(|| symbols.iter().position(|s| top(s)));
        if let Some(i) = hit {
            if !symbols[i].wraps.contains(&Wrap::Memo) {
                symbols[i].wraps.insert(0, Wrap::Memo);
            }
        }
    }
}

/// A dep like `addItem` from `const { addItem } = useCart()` is only stable if `useCart`
/// wraps it in useCallback / useMemo. Look inside the hook and re-classify.
fn check_hook_values(symbols: &mut [Symbol]) {
    // (hook name, function defined in it) -> memoized?
    let mut inside: HashMap<(String, String), bool> = HashMap::new();
    for s in symbols.iter() {
        if let Some(p) = s.parent {
            if symbols[p].kind == SymKind::Hook {
                inside.insert((symbols[p].name.clone(), s.name.clone()), s.is_memoized());
            }
        }
    }
    for s in symbols.iter_mut() {
        for info in s.deps.iter_mut().chain(s.dep_hooks.iter_mut()) {
            let Some(list) = info.deps.as_mut() else { continue };
            let mut extra = Vec::new();
            for d in list.iter_mut().filter(|d| d.origin == Origin::HookValue) {
                let (Some(hook), Some(root)) = (d.detail.clone(), d.root.clone()) else { continue };
                match inside.get(&(hook.clone(), root.clone())) {
                    Some(false) => {
                        d.origin = Origin::Unstable;
                        d.detail = Some("function".into());
                        extra.push(Hint {
                            warn: true,
                            text: format!(
                                "`{root}` comes from {hook}, which doesn't wrap it in useCallback, so it is a new function on every render and {}.",
                                rerun_phrase(&info.hook)
                            ),
                        });
                    }
                    Some(true) => d.detail = Some(format!("{hook} · memoized")),
                    None => {}
                }
            }
            info.hints.extend(extra);
        }
    }
}

pub fn build(root: &Path) -> Graph {
    // 1. list files (skipping heavy folders)
    let walker = WalkDir::new(root).into_iter().filter_entry(|e| {
        let n = e.file_name().to_string_lossy();
        !(n == "node_modules"
            || n == ".git"
            || n == "dist"
            || n == "build"
            || n == ".next"
            || n == "coverage"
            || n == ".turbo"
            || n == ".cache")
    });
    let mut paths: Vec<PathBuf> = walker
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            matches!(ext, "ts" | "tsx" | "js" | "jsx")
                && !p.to_string_lossy().ends_with(".d.ts")
                && !p.to_string_lossy().contains(".test.")
                && !p.to_string_lossy().contains(".spec.")
        })
        .collect();
    paths.sort();

    // 2. read + parse in parallel (file reads over WSL/network are slow)
    let nthreads = 16.min(paths.len().max(1));
    let chunk = paths.len().div_ceil(nthreads).max(1);
    let parts: Vec<Part> = std::thread::scope(|sc| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|c| {
                std::thread::Builder::new()
                    .stack_size(16 << 20)
                    .spawn_scoped(sc, move || parse_chunk(root, c))
                    .expect("spawn parser thread")
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("parser thread")).collect()
    });

    let mut symbols: Vec<Symbol> = Vec::new();
    let mut refs: Vec<RawRef> = Vec::new();
    let mut aliases: HashMap<(String, String), String> = HashMap::new();
    let mut memo_marks: Vec<(String, String)> = Vec::new();
    let mut comp_alias: HashMap<String, String> = HashMap::new();
    let mut modules: HashMap<String, Module> = HashMap::new();
    let mut files = 0;
    let mut file_list: Vec<String> = Vec::new();
    for part in parts {
        memo_marks.extend(part.memo_marks);
        comp_alias.extend(part.comp_alias);
        modules.extend(part.modules);
        let base = symbols.len();
        files += part.files;
        file_list.extend(part.file_list);
        symbols.extend(part.symbols.into_iter().map(|mut s| {
            s.parent = s.parent.map(|p| p + base);
            s
        }));
        refs.extend(part.refs.into_iter().map(|mut r| {
            r.owner += base;
            r
        }));
        aliases.extend(part.aliases);
    }

    // 3. cross-file facts about memoization
    apply_memo_marks(&mut symbols, &memo_marks);
    check_hook_values(&mut symbols);
    find_similar(&mut symbols);

    // 4. resolve references (indexed by name)
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, s) in symbols.iter().enumerate() {
        by_name.entry(s.name.as_str()).or_default().push(i);
    }
    // const FastCard = memo(Card): <FastCard /> renders Card
    for (alias, target) in &comp_alias {
        if !by_name.contains_key(alias.as_str()) {
            if let Some(v) = by_name.get(target.as_str()).cloned() {
                by_name.insert(alias.as_str(), v);
            }
        }
    }

    let resolver = Resolver::new(&symbols, &modules, &file_list);

    let mut edges: Vec<Edge> = Vec::new();
    let mut seen: HashSet<(usize, usize, EdgeKind)> = HashSet::new();
    // refs arrive in source order per file, so the first one kept is the first use
    let mut push = |from: usize, to: usize, kind: EdgeKind, line: usize, site: usize, edges: &mut Vec<Edge>| {
        if from != to && seen.insert((from, to, kind)) {
            edges.push(Edge { from, to, kind, line, site });
        }
    };

    for (i, s) in symbols.iter().enumerate() {
        if let Some(p) = s.parent {
            push(p, i, EdgeKind::Defines, s.line, p, &mut edges);
        }
    }

    for r in &refs {
        let owner = r.owner;
        let cands: &[usize] = by_name.get(r.name.as_str()).map(|v| v.as_slice()).unwrap_or(&[]);
        // 1. same file, visible from owner
        let target = cands
            .iter()
            .copied()
            .find(|&i| {
                i != owner
                    && symbols[i].file == r.file
                    && match symbols[i].parent {
                        None => true,
                        Some(p) => is_ancestor_or_self(&symbols, p, owner),
                    }
            })
            // 2. imported: follow the file's import (and re-exports / barrels) to its definition
            .or_else(|| resolver.resolve_local(&r.file, &r.name).filter(|&i| i != owner))
            // 3. destructured from a hook: const { addItem } = useCart()
            .or_else(|| {
                aliases.get(&(r.file.clone(), r.name.clone())).and_then(|hook| {
                    cands.iter().copied().find(|&i| {
                        symbols[i].parent.map(|p| &symbols[p].name == hook).unwrap_or(false)
                    })
                })
            })
            // 4. any top-level symbol with that name (unresolvable import: package, path alias, ...)
            .or_else(|| {
                cands
                    .iter()
                    .copied()
                    .find(|&i| symbols[i].parent.is_none() && i != owner)
            });
        let Some(t) = target else { continue };
        // rows={rows} passes a memoized value, not a handler; onChange={debounced} still counts
        if r.kind == EdgeKind::Handler && symbols[t].is_value && !r.event_prop {
            continue;
        }
        let kind = match (r.kind, symbols[t].kind) {
            (EdgeKind::Renders, SymKind::Component) => EdgeKind::Renders,
            (EdgeKind::Renders, _) => continue,
            (k, _) => k,
        };
        let from = match &r.via {
            None => owner,
            Some(v) => {
                let is_top_comp = |i: usize| symbols[i].kind == SymKind::Component && symbols[i].parent.is_none();
                let by_name_hit = |same_file: bool| {
                    by_name.get(v.as_str()).and_then(|c| {
                        c.iter().copied().find(|&i| is_top_comp(i) && (!same_file || symbols[i].file == r.file))
                    })
                };
                // imported component first, then one in the same file, then any with that name
                match resolver
                    .resolve_local(&r.file, v)
                    .filter(|&i| symbols[i].kind == SymKind::Component)
                    .or_else(|| by_name_hit(true))
                    .or_else(|| by_name_hit(false))
                {
                    Some(i) => i,
                    None => continue,
                }
            }
        };
        push(from, t, kind, r.line, owner, &mut edges);
    }

    let n = symbols.len();
    let mut out = vec![Vec::new(); n];
    let mut inc = vec![Vec::new(); n];
    for (ei, e) in edges.iter().enumerate() {
        out[e.from].push(ei);
        inc[e.to].push(ei);
    }
    file_list.sort();
    Graph { symbols, edges, files, file_list, out, inc }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Graph {
        build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/memo"))
    }

    fn sym<'g>(g: &'g Graph, name: &str) -> &'g Symbol {
        g.symbols.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no symbol {name}"))
    }

    fn warn_texts(info: &DepInfo) -> Vec<&str> {
        info.hints.iter().filter(|h| h.warn).map(|h| h.text.as_str()).collect()
    }

    #[test]
    fn wrapped_functions_become_symbols() {
        let g = fixture();
        assert_eq!(sym(&g, "ProductCard").wraps, vec![Wrap::Memo]);
        assert_eq!(sym(&g, "ProductCard").kind, SymKind::Component);
        assert_eq!(sym(&g, "SearchBox").wraps, vec![Wrap::ForwardRef]);
        assert_eq!(sym(&g, "handleAdd").wraps, vec![Wrap::UseCallback]);
        assert_eq!(sym(&g, "total").wraps, vec![Wrap::UseMemo]);
        assert_eq!(sym(&g, "clear").wraps, vec![Wrap::UseCallback]);
        // export const FastBadge = memo(Badge)
        assert!(sym(&g, "Badge").wraps.contains(&Wrap::Memo));
    }

    #[test]
    fn memo_alias_resolves_renders() {
        let g = fixture();
        let app = g.symbols.iter().position(|s| s.name == "App").unwrap();
        let badge = g.symbols.iter().position(|s| s.name == "Badge").unwrap();
        assert!(g.out_edges(app).any(|e| e.to == badge && e.kind == EdgeKind::Renders));
    }

    #[test]
    fn dep_origins_and_hints() {
        let g = fixture();

        // useCallback(() => setQuery(q), []) — setters are stable
        let hs = sym(&g, "handleSearch").deps.as_ref().unwrap();
        assert!(hs.hints.is_empty(), "{:?}", hs.hints);

        // addItem comes from useCart, where it isn't memoized
        let ha = sym(&g, "handleAdd").deps.as_ref().unwrap();
        let deps = ha.deps.as_ref().unwrap();
        assert_eq!(deps[0].origin, Origin::Unstable);
        assert_eq!(deps[1].origin, Origin::Prop);
        assert_eq!(ha.warnings(), 1, "{:?}", ha.hints);

        // useMemo without deps
        let label = sym(&g, "label").deps.as_ref().unwrap();
        assert!(label.deps.is_none());
        assert_eq!(label.warnings(), 1);

        // App's effect: `query` missing, `options` unstable
        let app = sym(&g, "App");
        assert_eq!(app.dep_hooks.len(), 1);
        let w = warn_texts(&app.dep_hooks[0]);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w.iter().any(|t| t.contains("`query` (state)")));
        assert!(w.iter().any(|t| t.contains("`options` is a new object")));

        // CartSummary: first effect misses `label`; second is clean, `clear` is memoized in useCart
        let cs = sym(&g, "CartSummary");
        assert_eq!(cs.dep_hooks.len(), 2);
        assert!(warn_texts(&cs.dep_hooks[0]).iter().any(|t| t.contains("`label`")));
        assert!(cs.dep_hooks[1].hints.is_empty(), "{:?}", cs.dep_hooks[1].hints);
        let clear = &cs.dep_hooks[1].deps.as_ref().unwrap()[1];
        assert_eq!(clear.origin, Origin::HookValue);
        assert_eq!(clear.origin_label(), "from useCart · memoized");

        // SearchBox: effect with no deps is informational, not a warning
        let sb = sym(&g, "SearchBox");
        assert_eq!(sb.dep_hooks.len(), 1);
        assert!(sb.dep_hooks[0].deps.is_none());
        assert_eq!(sb.dep_hooks[0].warnings(), 0);
    }

    fn idx(g: &Graph, file: &str, name: &str) -> usize {
        g.symbols
            .iter()
            .position(|s| s.name == name && s.file == file)
            .unwrap_or_else(|| panic!("no symbol {name} in {file}"))
    }

    /// Targets of `kind` edges leaving `from`, as "file:name".
    fn targets(g: &Graph, from: usize, kind: EdgeKind) -> Vec<String> {
        let mut v: Vec<String> = g
            .out_edges(from)
            .filter(|e| e.kind == kind)
            .map(|e| format!("{}:{}", g.symbols[e.to].file, g.symbols[e.to].name))
            .collect();
        v.sort();
        v
    }

    fn imports_fixture() -> Graph {
        build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/imports"))
    }

    #[test]
    fn same_name_resolves_through_imports() {
        let g = imports_fixture();
        // b/PageB imports ./Toolbar; the decoy a/Toolbar.tsx sorts first
        let page_b = idx(&g, "b/PageB.tsx", "PageB");
        assert_eq!(targets(&g, page_b, EdgeKind::Renders), ["b/Toolbar.tsx:Toolbar"]);
        // hooks too: save() calls the useThing imported from ./useThing
        let save = idx(&g, "b/PageB.tsx", "save");
        assert_eq!(targets(&g, save, EdgeKind::Calls), ["b/useThing.ts:useThing"]);
        // renamed import alongside a same-named local import
        let page_a = idx(&g, "a/PageA.tsx", "PageA");
        assert_eq!(targets(&g, page_a, EdgeKind::Renders), ["a/Toolbar.tsx:Toolbar", "b/Toolbar.tsx:Toolbar"]);
    }

    #[test]
    fn handler_edges_start_at_the_imported_component() {
        let g = imports_fixture();
        let b_toolbar = idx(&g, "b/Toolbar.tsx", "Toolbar");
        assert_eq!(targets(&g, b_toolbar, EdgeKind::Handler), ["b/PageB.tsx:save"]);
        let a_toolbar = idx(&g, "a/Toolbar.tsx", "Toolbar");
        assert!(targets(&g, a_toolbar, EdgeKind::Handler).is_empty());
    }

    #[test]
    fn barrels_default_exports_memo_aliases_and_lazy() {
        let g = imports_fixture();
        let uses = idx(&g, "c/Uses.tsx", "Uses");
        // Button via `export *`, Badge via `export { default as Badge }`, Fancy as a renamed
        // default import, FastCard via `export const FastCard = memo(Card)`
        assert_eq!(
            targets(&g, uses, EdgeKind::Renders),
            ["ui/Badge.tsx:Badge", "ui/Button.tsx:Button", "ui/Card.tsx:Card"]
        );
        // const Lazy = lazy(() => import("./LazyPage"))
        let app = idx(&g, "c/App.tsx", "App");
        assert_eq!(targets(&g, app, EdgeKind::Renders), ["c/LazyPage.tsx:LazyPage"]);
    }

    #[test]
    fn complex_react_toolbar_collision() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../complex-react");
        if !root.exists() {
            return;
        }
        let g = build(&root);
        let projects = idx(&g, "src/features/projects/ProjectsPage.tsx", "ProjectsPage");
        let board = idx(&g, "src/features/board/BoardPage.tsx", "BoardPage");
        let toolbar = |from| {
            targets(&g, from, EdgeKind::Renders).into_iter().filter(|t| t.ends_with(":Toolbar")).collect::<Vec<_>>()
        };
        assert_eq!(toolbar(projects), ["src/features/projects/Toolbar.tsx:Toolbar"]);
        assert_eq!(toolbar(board), ["src/features/board/Toolbar.tsx:Toolbar"]);
    }

    #[test]
    fn memoized_values_are_not_handlers() {
        let g = build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/handlers"));
        let f = "List.tsx";
        assert!(sym(&g, "rows").is_value && sym(&g, "options").is_value);
        assert!(!sym(&g, "format").is_value && !sym(&g, "render").is_value && !sym(&g, "sortRows").is_value);
        let table = idx(&g, f, "Table");
        assert_eq!(
            targets(&g, table, EdgeKind::Handler),
            [
                "List.tsx:format",   // useMemo returning an arrow
                "List.tsx:onSearch", // opaque useMemo result, but passed to an `on*` prop
                "List.tsx:render",   // useMemo block returning a function expression
                "List.tsx:sortRows", // useCallback
            ]
        );
    }

    #[test]
    fn complex_react_has_no_value_handlers() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../complex-react");
        if !root.exists() {
            return;
        }
        let g = build(&root);
        let bad: Vec<String> = g
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Handler && g.symbols[e.to].is_value)
            .map(|e| format!("{} -> {}", g.symbols[e.from].name, g.symbols[e.to].name))
            .collect();
        assert!(bad.is_empty(), "{bad:?}");
        // real handlers survive
        let column = idx(&g, "src/features/board/BoardColumn.tsx", "BoardColumn");
        assert!(targets(&g, column, EdgeKind::Handler).contains(&"src/features/board/BoardPage.tsx:handleQuickAdd".to_string()));
    }

    fn similar_to(g: &Graph, file: &str, name: &str) -> Vec<(String, bool)> {
        let i = idx(g, file, name);
        g.symbols[i]
            .similar
            .iter()
            .map(|m| (format!("{}:{}", g.symbols[m.other].file, g.symbols[m.other].name), m.exact))
            .collect()
    }

    #[test]
    fn similar_bodies_are_matched() {
        let g = build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/similar"));
        // renamed copy = identical structure; a copy with an extra line = similar, best match first
        assert_eq!(
            similar_to(&g, "ProductList.tsx", "ProductList"),
            [("OrderList.tsx:OrderList".to_string(), true), ("UserList.tsx:UserList".to_string(), false)]
        );
        let user = idx(&g, "UserList.tsx", "UserList");
        assert!(g.symbols[user].similar.iter().all(|m| !m.exact && m.score >= 0.8 && m.score < 1.0));
        // `function f() {}` and `const f = () => {}` with the same body match exactly
        assert_eq!(similar_to(&g, "ProductList.tsx", "formatPrice"), [("OrderList.tsx:formatTotal".to_string(), true)]);
        // unrelated code, tiny functions and nested handlers below the cutoff are not flagged
        for (f, n) in [("Settings.tsx", "Settings"), ("Settings.tsx", "double"), ("ProductList.tsx", "close")] {
            assert!(similar_to(&g, f, n).is_empty(), "{n}");
        }
    }

    #[test]
    fn similar_ignores_small_context_guards() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../complex-react");
        if !root.exists() {
            return;
        }
        let g = build(&root);
        // `const ctx = useContext(X); if (!ctx) throw ...; return ctx;` repeats, but is too small to flag
        assert!(similar_to(&g, "src/context/AuthContext.tsx", "useAuth").is_empty());
        // same-name components with different bodies are not duplicates
        assert!(similar_to(&g, "src/features/projects/Toolbar.tsx", "Toolbar").is_empty());
    }
}
