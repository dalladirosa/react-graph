//! Parses a React (TS/JS) folder into a graph of components, hooks and functions.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use crate::alias::AliasMap;
use crate::drill::{self, DrillHint};
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
    /// A component providing a context (`<Ctx.Provider>`) to a symbol that reads it
    /// (`useContext(Ctx)`, directly or through a custom hook). Not an execution step.
    Context,
}

impl EdgeKind {
    pub fn label(self) -> &'static str {
        match self {
            EdgeKind::Renders => "renders",
            EdgeKind::Calls => "calls",
            EdgeKind::Defines => "defines",
            EdgeKind::Handler => "handler",
            EdgeKind::Context => "context",
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

/// `<Card onSelect={() => ...} />` where `Card` is wrapped in `memo`: the prop is a new value on
/// every render, so the memo never gets to skip the re-render.
#[derive(Clone, Debug)]
pub struct PropHint {
    /// the memo component receiving the prop
    pub child: usize,
    pub prop: String,
    pub text: String,
    /// 1-based line of the prop
    pub line: usize,
}

/// A JSX prop whose value is created anew on every render (not yet matched to a memo component).
struct UnstableProp {
    prop: String,
    /// "object", "array", "function", "element" or "instance"
    what: &'static str,
    /// the render-body variable holding it, for `style={style}`; None for an inline literal
    via: Option<String>,
    line: usize,
}

impl UnstableProp {
    fn hint(&self, child: &str, id: usize) -> PropHint {
        let (prop, what) = (&self.prop, self.what);
        let fix = if what == "function" { "useCallback" } else { "useMemo" };
        let text = match &self.via {
            None => format!(
                "`{child}` is wrapped in memo, but `{prop}` gets a new {what} on every render, so memo never skips it. Hoist it out of the component or wrap it in {fix}."
            ),
            Some(n) => format!(
                "`{child}` is wrapped in memo, but `{prop}={{{n}}}` passes `{n}`, a new {what} created in the render body on every render, so memo never skips it. Wrap `{n}` in {fix}."
            ),
        };
        PropHint { child: id, prop: prop.clone(), text, line: self.line }
    }
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
    /// props this component passes to a `memo` component that are new on every render
    pub prop_hints: Vec<PropHint>,
    /// props this component only hands on to a child, as part of a long chain (prop drilling)
    pub drill_hints: Vec<DrillHint>,
    /// binds a computed value, not a function: `const rows = useMemo(() => filter(...), deps)`
    pub is_value: bool,
    /// other symbols of the same kind with an identical or near-identical body, best match first
    pub similar: Vec<Similar>,
    /// member of a render / call cycle: index into `Graph::cycles`
    pub cycle: Option<usize>,
    /// test files that import this symbol (indexes into `Graph::test_files`); top-level symbols only,
    /// see `Graph::tests_of`
    pub tests: Vec<usize>,
    /// byte range of the function node in its file (what `similar` fingerprints), for side-by-side compare
    pub func: Option<(usize, usize)>,
    /// structural fingerprint; only kept until similar bodies are matched
    shape: Shape,
    /// what the component does with its props; only kept until prop drilling is resolved
    pub(crate) forwards: Option<drill::Forwards>,
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
    /// warnings on this symbol's own deps and the hooks it calls, plus props that defeat a memo child
    pub fn warnings(&self) -> usize {
        self.deps.as_ref().map_or(0, |d| d.warnings())
            + self.dep_hooks.iter().map(|d| d.warnings()).sum::<usize>()
            + self.prop_hints.len()
            + self.drill_hints.len()
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
    /// `Context` edges: index into `Graph::contexts`
    pub ctx: Option<usize>,
}

/// A `createContext` declaration.
#[derive(Clone, Debug)]
pub struct ContextDecl {
    pub name: String,
    pub file: String,
}

#[derive(Default)]
pub struct Graph {
    pub symbols: Vec<Symbol>,
    pub edges: Vec<Edge>,
    pub files: usize,
    /// relative paths (forward slashes) of every parsed source file
    pub file_list: Vec<String>,
    /// relative paths of the test files (`*.test.*`, `*.spec.*`, `__tests__/`); read for their imports only
    pub test_files: Vec<String>,
    /// every `createContext` found, for `Edge::ctx`
    pub contexts: Vec<ContextDecl>,
    /// groups of symbols that render or call each other in a loop (A -> B -> A), members sorted
    pub cycles: Vec<Vec<usize>>,
    /// props that travel through components that only forward them (the hints on the symbols come from these)
    #[allow(dead_code)]
    pub drills: Vec<drill::Chain>,
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
    /// Test files that import `i`, or the top-level symbol it is defined in.
    pub fn tests_of(&self, i: usize) -> &[usize] {
        let mut top = i;
        while let Some(p) = self.symbols[top].parent {
            top = p;
        }
        &self.symbols[top].tests
    }
    /// A top-level symbol that no test file imports. Never true when the project has no tests at all.
    pub fn untested(&self, i: usize) -> bool {
        !self.test_files.is_empty() && self.symbols[i].parent.is_none() && self.symbols[i].tests.is_empty()
    }
    /// "renders", "calls", ... or "context · ThemeContext"
    pub fn edge_label(&self, e: &Edge) -> String {
        match e.ctx.and_then(|c| self.contexts.get(c)) {
            Some(c) => format!("{} · {}", e.kind.label(), c.name),
            None => e.kind.label().to_string(),
        }
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
    /// For renders: props with a value that is new on every render
    unstable: Vec<UnstableProp>,
}

/// `<Ctx.Provider>` / `useContext(Ctx)`: resolved to a `createContext` once all files are known.
struct RawCtx {
    owner: usize,
    name: String,
    file: String,
    line: usize,
    /// true = provides the context (`<Ctx.Provider>` / `<Ctx>`), false = reads it
    provide: bool,
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
    ctx_refs: &'a mut Vec<RawCtx>,
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
pub(crate) fn strip_expr(mut n: Node) -> Node {
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
pub(crate) fn pattern_names(n: Node, src: &str, out: &mut Vec<String>) {
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

pub(crate) fn text<'a>(n: Node, src: &'a str) -> &'a str {
    n.utf8_text(src.as_bytes()).unwrap_or("")
}

/// Props of a JSX element whose value is a new object / array / function / element on every
/// render: written inline (`style={{ ... }}`) or bound so in the render body (`style={style}`
/// after `const style = { ... }`). `key` and `ref` never reach the component as props.
fn unstable_props(el: Node, scopes: &[Scope], src: &str) -> Vec<UnstableProp> {
    let mut out = Vec::new();
    let mut c = el.walk();
    for a in el.children(&mut c).filter(|a| a.kind() == "jsx_attribute") {
        let Some(prop) = a.named_child(0).filter(|p| p.kind() == "property_identifier").map(|p| text(p, src)) else {
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
        let v = strip_expr(e.named_child(0).unwrap());
        let (what, via) = match v.kind() {
            "object" => ("object", None),
            "array" => ("array", None),
            "arrow_function" | "function_expression" | "function" => ("function", None),
            "jsx_element" | "jsx_self_closing_element" | "jsx_fragment" => ("element", None),
            "identifier" => match lookup(scopes, text(v, src)) {
                Some((Origin::Unstable, Some(d))) => {
                    let what = match d.as_str() {
                        "object" => "object",
                        "array" => "array",
                        "function" => "function",
                        "JSX element" => "element",
                        _ => "instance",
                    };
                    (what, Some(text(v, src).to_string()))
                }
                _ => continue,
            },
            _ => continue,
        };
        out.push(UnstableProp { prop: prop.to_string(), what, via, line: a.start_position().row + 1 });
    }
    out
}

/// An object property that puts a component on a route (react-router and similar):
/// `element: <Home />`, `errorElement: <NotFound />`, `Component: Home`, `ErrorBoundary: NotFound`.
/// Returns the key and the value node (a JSX element, or an uppercase identifier).
fn route_entry<'t>(pair: Node<'t>, src: &str) -> Option<(&'static str, Node<'t>)> {
    let key = pair.child_by_field_name("key")?;
    let key = text(key, src).trim_matches(|c| c == '"' || c == '\'');
    let value = strip_expr(pair.child_by_field_name("value")?);
    let jsx = matches!(value.kind(), "jsx_element" | "jsx_self_closing_element");
    let ident = value.kind() == "identifier" && text(value, src).starts_with(|c: char| c.is_uppercase());
    ["element", "errorElement", "Component", "ErrorBoundary"]
        .into_iter()
        .find(|k| *k == key && match *k {
            "element" | "errorElement" => jsx,
            _ => ident,
        })
        .map(|k| (k, value))
}

/// Does this module-level expression hold route entries (see `route_entry`)?
fn has_route_entries(node: Node, src: &str) -> bool {
    if node.kind() == "pair" && route_entry(node, src).is_some() {
        return true;
    }
    let mut c = node.walk();
    // functions are their own symbols: routes defined inside one belong to it
    node.children(&mut c).filter(|ch| !is_fn(*ch)).any(|ch| has_route_entries(ch, src))
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
                // module-level route config, rendered by no component:
                //   const router = createBrowserRouter([{ path: "/", element: <Home /> }])
                // the config renders its pages, so it gets a symbol of its own to hold those links
                if owner.is_none() && name.kind() == "identifier" && has_route_entries(value, cx.src) {
                    let id = new_symbol(text(name, cx.src), node, None, cx);
                    cx.symbols[id].kind = SymKind::Component;
                    visit(value, Some(id), cx);
                    return;
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
        // route objects: `{ Component: Home }` renders Home just like `{ element: <Home /> }`
        "pair" => {
            if let (Some(owner), Some((key, value))) = (owner, route_entry(node, cx.src)) {
                if value.kind() == "identifier" && matches!(key, "Component" | "ErrorBoundary") {
                    cx.refs.push(RawRef {
                        owner,
                        name: text(value, cx.src).to_string(),
                        kind: EdgeKind::Renders,
                        file: cx.file.clone(),
                        line: node.start_position().row + 1,
                        via: None,
                        event_prop: false,
                        unstable: Vec::new(),
                    });
                }
            }
        }
        "jsx_opening_element" | "jsx_self_closing_element" => {
            if let (Some(owner), Some(name)) = (owner, node.child_by_field_name("name")) {
                let n = text(name, cx.src);
                // <Ctx.Provider value=...> / <Ctx.Consumer> / React 19's <Ctx value=...>
                let ctx_use = match name.kind() {
                    "member_expression" => match (name.child_by_field_name("object"), name.child_by_field_name("property")) {
                        (Some(o), Some(p)) if o.kind() == "identifier" => match text(p, cx.src) {
                            "Provider" => Some((text(o, cx.src), true)),
                            "Consumer" => Some((text(o, cx.src), false)),
                            _ => None,
                        },
                        _ => None,
                    },
                    "identifier" if n.starts_with(|c: char| c.is_uppercase()) => Some((n, true)),
                    _ => None,
                };
                if let Some((ctx, provide)) = ctx_use {
                    cx.ctx_refs.push(RawCtx {
                        owner,
                        name: ctx.to_string(),
                        file: cx.file.clone(),
                        line: node.start_position().row + 1,
                        provide,
                    });
                }
                if n.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                    cx.refs.push(RawRef {
                        owner,
                        name: n.to_string(),
                        kind: EdgeKind::Renders,
                        file: cx.file.clone(),
                        line: node.start_position().row + 1,
                        via: None,
                        event_prop: false,
                        unstable: unstable_props(node, &cx.scopes, cx.src),
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
                                        unstable: Vec::new(),
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
            // useContext(Ctx) / use(Ctx)
            if let (Some(owner), Some(h)) = (owner, callee) {
                if matches!(h, "useContext" | "use") {
                    if let Some(a) = call_args(node).first().map(|a| strip_expr(*a)).filter(|a| a.kind() == "identifier") {
                        cx.ctx_refs.push(RawCtx {
                            owner,
                            name: text(a, cx.src).to_string(),
                            file: cx.file.clone(),
                            line: node.start_position().row + 1,
                            provide: false,
                        });
                    }
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
                        unstable: Vec::new(),
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
        prop_hints: Vec::new(),
        drill_hints: Vec::new(),
        is_value: false,
        similar: Vec::new(),
        cycle: None,
        tests: Vec::new(),
        func: None,
        shape: Shape::default(),
        forwards: None,
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
        let scope = build_scope(func, kind, cx.src);
        if kind == SymKind::Component {
            let names: HashSet<String> = scope.bindings.keys().cloned().collect();
            // `const C = (props) => ...` is entered with the function itself as its body: the parameters aren't reads
            let real_body = func.child_by_field_name("body").unwrap_or(body);
            cx.symbols[id].forwards = Some(drill::analyze(func, real_body, &names, cx.src));
        }
        cx.scopes.push(scope);
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

impl Import {
    fn name_is_namespace(&self) -> bool {
        matches!(self.name, Imported::Namespace)
    }
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
    /// top-level `const ThemeContext = createContext(...)`
    contexts: HashMap<String, usize>,
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

/// `createContext(...)` / `React.createContext<T>(...)`
fn is_create_context(value: Node, src: &str) -> bool {
    let v = strip_expr(value);
    v.kind() == "call_expression" && callee_name(v, src) == Some("createContext")
}

/// Top-level `const` declarations: lazy imports, memo aliases and contexts.
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
        if is_create_context(value, src) {
            m.contexts.insert(name, d.start_position().row + 1);
        } else if let Some(spec) = lazy_import_spec(value, src) {
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
    /// tsconfig / jsconfig `paths` and `baseUrl`
    aliases: &'a AliasMap,
}

impl<'a> Resolver<'a> {
    fn new(
        symbols: &'a [Symbol],
        modules: &'a HashMap<String, Module>,
        file_list: &'a [String],
        aliases: &'a AliasMap,
    ) -> Self {
        let mut top: HashMap<&str, HashMap<&str, usize>> = HashMap::new();
        for (i, s) in symbols.iter().enumerate().filter(|(_, s)| s.parent.is_none()) {
            top.entry(s.file.as_str()).or_default().entry(s.name.as_str()).or_insert(i);
        }
        Resolver { modules, files: file_list.iter().map(|f| f.as_str()).collect(), top, aliases }
    }

    fn find_file(&self, base: &str) -> Option<&'a str> {
        RESOLVE_SUFFIXES.iter().find_map(|sfx| self.files.get(format!("{base}{sfx}").as_str()).copied())
    }

    /// `"../ui"` imported from `src/a/B.tsx` => `src/ui/index.ts` (if it was parsed).
    /// `"@/ui/Button"` goes through the tsconfig `paths`; packages resolve to nothing.
    fn resolve_spec(&self, from: &str, spec: &str) -> Option<&'a str> {
        if !spec.starts_with('.') {
            return self.aliases.candidates(spec).iter().find_map(|base| self.find_file(base));
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
        self.find_file(&parts.join("/"))
    }

    /// Follows `name` exported by `file` (through aliases, imports and re-exports) to the place
    /// where `leaf(file, local_name)` finds it.
    fn find_export_with<T>(&self, file: &str, name: &str, depth: usize, leaf: &dyn Fn(&str, &str) -> Option<T>) -> Option<T> {
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
            if let Some(hit) = leaf(file, target) {
                return Some(hit);
            }
            // import { X } from "./x"; export { X };
            if let Some(imp) = m.imports.get(target) {
                return self.resolve_import_with(file, imp, depth + 1, leaf);
            }
        }
        m.reexports.iter().find_map(|re| match re {
            ReExport::Star { spec } if name != "default" => {
                self.resolve_spec(file, spec).and_then(|f| self.find_export_with(f, name, depth + 1, leaf))
            }
            ReExport::Named { spec, imported, exported } if exported == name => {
                self.resolve_spec(file, spec).and_then(|f| self.find_export_with(f, imported, depth + 1, leaf))
            }
            _ => None,
        })
    }

    fn resolve_import_with<T>(&self, file: &str, imp: &Import, depth: usize, leaf: &dyn Fn(&str, &str) -> Option<T>) -> Option<T> {
        let f = self.resolve_spec(file, &imp.spec)?;
        match &imp.name {
            Imported::Named(n) => self.find_export_with(f, n, depth, leaf),
            Imported::Default => self.find_export_with(f, "default", depth, leaf),
            Imported::Namespace => None,
        }
    }

    fn resolve_import(&self, file: &str, imp: &Import, depth: usize) -> Option<usize> {
        self.resolve_import_with(file, imp, depth, &|f, t| self.top.get(f).and_then(|m| m.get(t)).copied())
    }

    /// The symbol `name`, as used in `file`, refers to through that file's imports.
    fn resolve_local(&self, file: &str, name: &str) -> Option<usize> {
        let imp = self.modules.get(file)?.imports.get(name)?;
        self.resolve_import(file, imp, 0)
    }

    /// The `createContext` declaration `name` refers to in `file`: declared there, or imported.
    fn resolve_context(&self, file: &str, name: &str, ids: &HashMap<(&str, &str), usize>) -> Option<usize> {
        if let Some(&i) = ids.get(&(file, name)) {
            return Some(i);
        }
        let imp = self.modules.get(file)?.imports.get(name)?;
        self.resolve_import_with(file, imp, 0, &|f, t| ids.get(&(f, t)).copied())
    }
}

struct Part {
    symbols: Vec<Symbol>,
    refs: Vec<RawRef>,
    ctx_refs: Vec<RawCtx>,
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
        ctx_refs: Vec::new(),
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
            ctx_refs: &mut part.ctx_refs,
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

/// Strongly connected components with more than one node (Tarjan, iterative: no recursion
/// depth to overflow on a long chain), each sorted, in order of their first member.
fn find_cycles(adj: &[Vec<usize>]) -> Vec<Vec<usize>> {
    const NONE: usize = usize::MAX;
    let n = adj.len();
    let (mut index, mut low) = (vec![NONE; n], vec![0; n]);
    let mut on_stack = vec![false; n];
    let (mut stack, mut found) = (Vec::new(), Vec::new());
    let mut next = 0;
    for root in 0..n {
        if index[root] != NONE {
            continue;
        }
        // (node, next neighbour to look at)
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some(&(v, i)) = work.last() {
            if index[v] == NONE {
                index[v] = next;
                low[v] = next;
                next += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            if let Some(&w) = adj[v].get(i) {
                work.last_mut().unwrap().1 += 1;
                if index[w] == NONE {
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                let mut comp = Vec::new();
                loop {
                    let w = stack.pop().unwrap();
                    on_stack[w] = false;
                    comp.push(w);
                    if w == v {
                        break;
                    }
                }
                if comp.len() > 1 {
                    comp.sort_unstable();
                    found.push(comp);
                }
            }
        }
    }
    found.sort();
    found
}

/// Links the component providing a context to every symbol reading it: the ones calling
/// `useContext(Ctx)` and, through custom hooks, the components and hooks that call those.
fn add_context_edges(
    symbols: &[Symbol],
    modules: &HashMap<String, Module>,
    resolver: &Resolver,
    ctx_refs: &[RawCtx],
    edges: &mut Vec<Edge>,
) -> Vec<ContextDecl> {
    // declarations, in file order for a stable result
    let mut decls: Vec<ContextDecl> = Vec::new();
    let mut files: Vec<&String> = modules.keys().collect();
    files.sort();
    for f in files {
        let mut names: Vec<&String> = modules[f].contexts.keys().collect();
        names.sort();
        decls.extend(names.into_iter().map(|n| ContextDecl { name: n.clone(), file: f.clone() }));
    }
    if decls.is_empty() {
        return decls;
    }
    let ids: HashMap<(&str, &str), usize> =
        decls.iter().enumerate().map(|(i, d)| ((d.file.as_str(), d.name.as_str()), i)).collect();
    // an import we could not follow (package, odd alias): accept a context name that is unique
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, d) in decls.iter().enumerate() {
        by_name.entry(d.name.as_str()).or_default().push(i);
    }

    // context -> (providers, readers), each with the line of the first use
    type Users = Vec<(usize, usize)>;
    let mut users: Vec<(Users, Users)> = vec![(Vec::new(), Vec::new()); decls.len()];
    for r in ctx_refs {
        let Some(c) = resolver
            .resolve_context(&r.file, &r.name, &ids)
            .or_else(|| by_name.get(r.name.as_str()).filter(|v| v.len() == 1).map(|v| v[0]))
        else {
            continue;
        };
        let side = if r.provide { &mut users[c].0 } else { &mut users[c].1 };
        if !side.iter().any(|&(o, _)| o == r.owner) {
            side.push((r.owner, r.line));
        }
    }

    // callers by callee, to carry a hook's context on to whoever calls the hook
    let mut callers: HashMap<usize, Vec<usize>> = HashMap::new();
    for e in edges.iter().filter(|e| e.kind == EdgeKind::Calls) {
        callers.entry(e.to).or_default().push(e.from);
    }

    let mut seen: HashSet<(usize, usize, usize)> = HashSet::new();
    for (c, (providers, readers)) in users.iter().enumerate() {
        // readers, plus whoever reads through a custom hook (stops at components)
        let mut all: Vec<usize> = readers.iter().map(|&(o, _)| o).collect();
        let mut k = 0;
        while k < all.len() {
            let r = all[k];
            k += 1;
            if symbols[r].kind != SymKind::Hook {
                continue;
            }
            for &caller in callers.get(&r).map(Vec::as_slice).unwrap_or(&[]) {
                if !all.contains(&caller) && symbols[caller].kind != SymKind::Function {
                    all.push(caller);
                }
            }
        }
        for &(p, line) in providers {
            for &to in &all {
                if p != to && seen.insert((p, to, c)) {
                    edges.push(Edge { from: p, to, kind: EdgeKind::Context, line, site: p, ctx: Some(c) });
                }
            }
        }
    }
    decls
}

/// Folders that never hold the project's own source.
fn is_skipped_dir(name: &str) -> bool {
    matches!(name, "node_modules" | ".git" | "dist" | "build" | ".next" | "coverage" | ".turbo" | ".cache")
}

fn is_script(p: &Path) -> bool {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(ext, "ts" | "tsx" | "js" | "jsx") && !p.to_string_lossy().ends_with(".d.ts")
}

/// A test file: `Card.test.tsx`, `Card.spec.js`, or anything inside a `__tests__` folder.
pub fn is_test_file(p: &Path) -> bool {
    let s = p.to_string_lossy();
    is_script(p)
        && (s.contains(".test.") || s.contains(".spec.") || p.components().any(|c| c.as_os_str() == "__tests__"))
}

/// A file `build` parses into symbols (test files are only read for what they import).
fn is_source_file(p: &Path) -> bool {
    is_script(p) && !is_test_file(p)
}

/// Would a change to `p` change what `build` returns: a source file, or the tsconfig that
/// decides how imports resolve, outside the skipped folders. For the file watcher.
pub fn affects_graph(p: &Path) -> bool {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    (is_script(p) || matches!(name, "tsconfig.json" | "jsconfig.json"))
        && !p.components().any(|c| is_skipped_dir(&c.as_os_str().to_string_lossy()))
}

/// Imports of each test file (relative path, module facts), in path order.
fn parse_test_modules(root: &Path, paths: &[PathBuf]) -> Vec<(String, Module)> {
    if paths.is_empty() {
        return Vec::new();
    }
    let nthreads = 8.min(paths.len());
    let chunk = paths.len().div_ceil(nthreads).max(1);
    let parts: Vec<Vec<(String, Module)>> = std::thread::scope(|sc| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|c| {
                std::thread::Builder::new()
                    .stack_size(16 << 20)
                    .spawn_scoped(sc, move || {
                        let mut parser = Parser::new();
                        parser.set_language(&tree_sitter_typescript::LANGUAGE_TSX.into()).expect("load tsx grammar");
                        c.iter()
                            .filter_map(|p| {
                                let src = std::fs::read_to_string(p).ok()?;
                                let tree = parser.parse(&src, None)?;
                                let rel = p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/");
                                Some((rel, collect_module(tree.root_node(), &src)))
                            })
                            .collect()
                    })
                    .expect("spawn parser thread")
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("parser thread")).collect()
    });
    parts.into_iter().flatten().collect()
}

/// (symbol, test file index) for every symbol a test file imports. `import * as X from "./x"`
/// counts for every top-level symbol of that file.
fn test_links(
    symbols: &[Symbol],
    modules: &HashMap<String, Module>,
    file_list: &[String],
    aliases: &AliasMap,
    test_files: &[String],
) -> Vec<(usize, usize)> {
    let resolver = Resolver::new(symbols, modules, file_list, aliases);
    let mut out = Vec::new();
    for (t, file) in test_files.iter().enumerate() {
        let Some(m) = modules.get(file) else { continue };
        let mut names: Vec<&String> = m.imports.keys().collect();
        names.sort();
        for name in names {
            let imp = &m.imports[name];
            if imp.name_is_namespace() {
                if let Some(f) = resolver.resolve_spec(file, &imp.spec) {
                    out.extend(symbols.iter().enumerate().filter(|(_, s)| s.parent.is_none() && s.file == f).map(|(i, _)| (i, t)));
                }
            } else if let Some(i) = resolver.resolve_import(file, imp, 0) {
                out.push((i, t));
            }
        }
    }
    out
}

pub fn build(root: &Path) -> Graph {
    // 1. list files (skipping heavy folders)
    let walker = WalkDir::new(root).into_iter().filter_entry(|e| !is_skipped_dir(&e.file_name().to_string_lossy()));
    let all: Vec<PathBuf> =
        walker.filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()).map(|e| e.into_path()).collect();
    // judged by the path inside the project: a folder called `my.test.app` or `__tests__` above it says nothing
    let inside = |p: &PathBuf| p.strip_prefix(root).unwrap_or(p).to_path_buf();
    let mut paths: Vec<PathBuf> = all.iter().filter(|p| is_source_file(&inside(p))).cloned().collect();
    let mut test_paths: Vec<PathBuf> = all.into_iter().filter(|p| is_test_file(&inside(p))).collect();
    paths.sort();
    test_paths.sort();

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
    let mut ctx_refs: Vec<RawCtx> = Vec::new();
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
        ctx_refs.extend(part.ctx_refs.into_iter().map(|mut r| {
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

    let alias_map = AliasMap::load(root);
    let resolver = Resolver::new(&symbols, &modules, &file_list, &alias_map);

    let mut edges: Vec<Edge> = Vec::new();
    let mut prop_hints: Vec<(usize, PropHint)> = Vec::new();
    let mut hops: Vec<drill::Hop> = Vec::new();
    let mut seen: HashSet<(usize, usize, EdgeKind)> = HashSet::new();
    // refs arrive in source order per file, so the first one kept is the first use
    let mut push = |from: usize, to: usize, kind: EdgeKind, line: usize, site: usize, edges: &mut Vec<Edge>| {
        if from != to && seen.insert((from, to, kind)) {
            edges.push(Edge { from, to, kind, line, site, ctx: None });
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
        // memo only helps when the props keep their identity
        if kind == EdgeKind::Renders && t != owner && symbols[t].wraps.contains(&Wrap::Memo) {
            prop_hints.extend(r.unstable.iter().map(|u| (owner, u.hint(&symbols[t].name, t))));
        }
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
        // `<Child prop={own} />`: the value goes on to `t`
        if kind == EdgeKind::Renders && r.via.is_none() && t != owner {
            if let Some(f) = &symbols[owner].forwards {
                hops.extend(f.passes.iter().filter(|p| p.child == r.name && p.elem_line == r.line).map(|p| drill::Hop {
                    from: owner,
                    to: t,
                    own: p.own.clone(),
                    child_prop: p.child_prop.clone(),
                    line: p.line,
                }));
            }
        }
    }

    let contexts = add_context_edges(&symbols, &modules, &resolver, &ctx_refs, &mut edges);
    drop(resolver);

    // test files: which symbols they import (through barrels and path aliases, like any other import)
    let test_mods = parse_test_modules(root, &test_paths);
    let test_files: Vec<String> = test_mods.iter().map(|(f, _)| f.clone()).collect();
    modules.extend(test_mods);
    let links = test_links(&symbols, &modules, &file_list, &alias_map, &test_files);
    for (sym, t) in links {
        if !symbols[sym].tests.contains(&t) {
            symbols[sym].tests.push(t);
        }
    }

    for (owner, h) in prop_hints {
        let have = &mut symbols[owner].prop_hints;
        if !have.iter().any(|x| (x.child, &x.prop, x.line) == (h.child, &h.prop, h.line)) {
            have.push(h);
        }
    }

    // loops of renders / calls; a symbol calling itself is plain recursion and doesn't count
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); symbols.len()];
    for e in edges.iter().filter(|e| matches!(e.kind, EdgeKind::Renders | EdgeKind::Calls)) {
        adj[e.from].push(e.to);
    }
    let drills = drill::find_chains(&symbols, &hops);
    drill::add_hints(&mut symbols, &hops, &drills);
    for s in symbols.iter_mut() {
        s.forwards = None;
    }
    let cycles = find_cycles(&adj);
    for (c, members) in cycles.iter().enumerate() {
        for &m in members {
            symbols[m].cycle = Some(c);
        }
    }

    let n = symbols.len();
    let mut out = vec![Vec::new(); n];
    let mut inc = vec![Vec::new(); n];
    for (ei, e) in edges.iter().enumerate() {
        out[e.from].push(ei);
        inc[e.to].push(ei);
    }
    file_list.sort();
    Graph { symbols, edges, files, file_list, test_files, contexts, cycles, drills, out, inc }
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
    fn tsconfig_paths_resolve_aliased_imports() {
        let g = build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/aliases"));
        // "@/ui/Button" and "@shared/Card" via `paths`; the decoys under legacy/ sort first
        let home = idx(&g, "src/pages/Home.tsx", "Home");
        assert_eq!(
            targets(&g, home, EdgeKind::Renders),
            ["packages/shared/Card.tsx:Card", "src/ui/Button.tsx:Button"]
        );
        // "@ui" is an exact pattern pointing at a barrel
        let about = idx(&g, "src/pages/About.tsx", "About");
        assert_eq!(targets(&g, about, EdgeKind::Renders), ["src/ui/Button.tsx:Button"]);
        // nothing renders the legacy decoys
        assert_eq!(g.in_edges(idx(&g, "legacy/Button.tsx", "Button")).count(), 0);
    }

    #[test]
    fn context_providers_link_to_their_readers() {
        let g = build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/context"));
        let app = idx(&g, "App.tsx", "App");
        // Footer calls useContext itself, Header goes through useTheme (which is linked too);
        // Plain reads nothing, and legacy/Old reads a different context of the same name
        assert_eq!(
            targets(&g, app, EdgeKind::Context),
            ["Footer.tsx:Footer", "Header.tsx:Header", "useTheme.ts:useTheme"]
        );
        assert!(g.out_edges(app).filter(|e| e.kind == EdgeKind::Context).all(|e| {
            g.edge_label(e) == "context · ThemeContext" && g.contexts[e.ctx.unwrap()].file == "ThemeContext.ts"
        }));
        assert_eq!(g.in_edges(idx(&g, "legacy/Old.tsx", "Old")).filter(|e| e.kind == EdgeKind::Context).count(), 0);
        // the link is not a call or a render
        assert_eq!(targets(&g, app, EdgeKind::Renders), ["Footer.tsx:Footer", "Header.tsx:Header", "Plain.tsx:Plain"]);
    }

    #[test]
    fn render_and_call_loops_are_cycles() {
        let g = build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cycles"));
        let cyc = |name: &str| sym(&g, name).cycle;
        // A -> B -> C -> A, and the even/odd pair
        let abc = cyc("A");
        assert!(abc.is_some() && cyc("B") == abc && cyc("C") == abc);
        let eo = cyc("isEven");
        assert!(eo.is_some() && cyc("isOdd") == eo && eo != abc);
        // renders the loop from outside, renders itself, and is a leaf: none are part of a loop
        assert!(cyc("Entry").is_none() && cyc("Tree").is_none() && cyc("Leaf").is_none());
        assert_eq!(g.cycles.len(), 2);
        let names: Vec<&str> = g.cycles[abc.unwrap()].iter().map(|&i| g.symbols[i].name.as_str()).collect();
        assert_eq!(names, ["A", "B", "C"]);
    }

    #[test]
    fn cycle_finder_handles_long_chains_and_self_loops() {
        // a 50k-node ring would overflow a recursive walk; a lone self-loop is not a cycle
        let n = 50_000;
        let mut adj: Vec<Vec<usize>> = (0..n).map(|i| vec![(i + 1) % n]).collect();
        adj.push(vec![n]);
        let found = find_cycles(&adj);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].len(), n);
    }

    #[test]
    fn only_graph_inputs_count_as_changes() {
        let hit = |p: &str| affects_graph(Path::new(p));
        assert!(hit("src/App.tsx") && hit("C:/proj/src/a/b.js") && hit("tsconfig.json") && hit("web/jsconfig.json"));
        // test files change which symbols count as tested
        assert!(hit("src/App.test.tsx") && hit("src/__tests__/a.ts"));
        assert!(!hit("README.md") && !hit("src/types.d.ts") && !hit("package.json"));
        assert!(!hit("node_modules/react/index.js") && !hit("C:/proj/dist/app.js") && !hit(".git/HEAD"));
    }

    #[test]
    fn unstable_props_to_memo_children_are_flagged() {
        let g = build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/memoprops"));
        let list = &g.symbols[idx(&g, "List.tsx", "List")];
        let hints: Vec<(&str, usize)> = list.prop_hints.iter().map(|h| (h.prop.as_str(), h.line)).collect();
        // inline function, `style` (object from the render body), `select` (function), inline array;
        // useCallback / useMemo values and the non-memo <Plain> are fine
        assert_eq!(hints, [("onSelect", 12), ("style", 13), ("onSelect", 14), ("options", 16)]);
        assert_eq!(list.warnings(), 4);
        let card = idx(&g, "Card.tsx", "Card");
        assert!(list.prop_hints.iter().all(|h| h.child == card));
        let texts: Vec<&str> = list.prop_hints.iter().map(|h| h.text.as_str()).collect();
        assert!(texts[0].contains("`Card` is wrapped in memo") && texts[0].contains("useCallback"), "{}", texts[0]);
        assert!(texts[1].contains("`style={style}`") && texts[1].contains("new object") && texts[1].contains("useMemo"), "{}", texts[1]);
        assert!(texts[3].contains("new array"), "{}", texts[3]);
        // nothing else is flagged
        assert_eq!(g.symbols.iter().map(|s| s.prop_hints.len()).sum::<usize>(), 4);
    }

    #[test]
    fn module_level_route_config_renders_its_pages() {
        let g = build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/routes"));
        // `export const router = createBrowserRouter([...])` becomes an entry point of its own
        let router = idx(&g, "router.tsx", "router");
        assert_eq!(g.symbols[router].kind, SymKind::Component);
        assert_eq!(
            targets(&g, router, EdgeKind::Renders),
            [
                "Layout.tsx:Layout",
                "pages/About.tsx:About",
                "pages/Home.tsx:Home",
                "pages/NotFound.tsx:NotFound",
                "pages/Settings.tsx:Settings", // `Component: Settings`, no JSX
            ]
        );
        assert_eq!(g.in_edges(router).count(), 0);
        // a page is no longer "unused", but a component in no route still is
        assert_eq!(g.in_edges(idx(&g, "pages/Home.tsx", "Home")).count(), 1);
        assert_eq!(g.in_edges(idx(&g, "Orphan.tsx", "Orphan")).count(), 0);
        // plain data and a lone `render(<App />)` don't turn into symbols
        assert!(g.symbols.iter().all(|s| s.name != "theme"));
        assert_eq!(g.symbols.len(), 7);
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

    fn drilling() -> Graph {
        build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drilling"))
    }

    fn drill_names(g: &Graph, c: &drill::Chain) -> Vec<String> {
        c.through.iter().map(|&t| g.symbols[t].name.clone()).collect()
    }

    #[test]
    fn prop_drilling_chains_are_found() {
        let g = drilling();
        let mut chains: Vec<(String, Vec<String>, String)> = g
            .drills
            .iter()
            .map(|c| (g.symbols[c.origin].name.clone(), drill_names(&g, c), g.symbols[c.end].name.clone()))
            .collect();
        chains.sort();
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            chains,
            [
                // useState value in App, forwarded by three components, read by Avatar
                ("App".to_string(), s(&["Layout", "Sidebar", "UserMenu"]), "Avatar".to_string()),
                // `props.items` style
                ("Dashboard".to_string(), s(&["Panel", "Table"]), "Row".to_string()),
            ]
        );
    }

    #[test]
    fn forwarding_components_get_a_warning() {
        let g = drilling();
        for n in ["Layout", "Sidebar", "UserMenu", "Panel", "Table"] {
            let s = sym(&g, n);
            assert_eq!(s.drill_hints.len(), 1, "{n}: {:?}", s.drill_hints);
            assert_eq!(s.warnings(), 1, "{n}");
        }
        let h = &sym(&g, "Sidebar").drill_hints[0];
        assert_eq!(h.prop, "user");
        assert!(h.text.contains("App \u{2192} Layout \u{2192} Sidebar \u{2192} UserMenu \u{2192} Avatar"), "{}", h.text);
        // the line of `<UserMenu user={user} />`
        assert_eq!(h.line, 9);
        // `theme` is forwarded one step to a component that reads it: no chain
        assert!(sym(&g, "Sidebar").drill_hints.iter().all(|h| h.prop != "theme"));
        // origin and the component that finally uses it are not flagged
        for n in ["App", "Avatar", "Dashboard", "Row"] {
            assert!(sym(&g, n).drill_hints.is_empty(), "{n}");
        }
    }

    #[test]
    fn short_chains_and_used_props_are_not_drilling() {
        let g = drilling();
        // one forwarding level is normal
        for n in ["Shallow", "Badge"] {
            assert!(sym(&g, n).drill_hints.is_empty(), "{n}");
        }
        // Toolbar reads `title` itself, so only Menu forwards it: one level
        for n in ["Toolbar", "Menu", "Item"] {
            assert!(sym(&g, n).drill_hints.is_empty(), "{n}");
        }
    }

    #[test]
    fn complex_react_drilling_is_consistent() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../complex-react");
        if !root.exists() {
            return;
        }
        let g = build(&root);
        for c in &g.drills {
            assert!(c.through.len() >= drill::MIN_THROUGH);
        }
    }

    #[test]
    fn arrow_function_components_drill_too() {
        let g = build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drilling_arrow"));
        // `(props) => <Table items={props.items} />`, with and without a block body
        assert_eq!(g.drills.len(), 1, "{:?}", g.drills);
        assert_eq!(drill_names(&g, &g.drills[0]), ["Panel", "Table"]);
        assert_eq!(sym(&g, "Dash").drill_hints.len(), 0);
        assert_eq!(sym(&g, "Panel").drill_hints.len(), 1);
        assert_eq!(sym(&g, "Row").drill_hints.len(), 0);
    }

    fn tests_fixture() -> Graph {
        build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tests"))
    }

    fn test_names(g: &Graph, name: &str) -> Vec<String> {
        let i = g.symbols.iter().position(|s| s.name == name).unwrap_or_else(|| panic!("no symbol {name}"));
        let mut v: Vec<String> = g.tests_of(i).iter().map(|&t| g.test_files[t].clone()).collect();
        v.sort();
        v
    }

    #[test]
    fn test_files_are_found_but_not_parsed_as_source() {
        let g = tests_fixture();
        assert_eq!(
            g.test_files,
            ["Card.test.tsx", "__tests__/Button.test.tsx", "__tests__/helpers.tsx", "hooks/useCounter.spec.ts", "math.test.ts"]
        );
        // neither tests nor helpers inside `__tests__` become symbols or files of the graph
        assert!(g.symbols.iter().all(|s| !s.file.contains("test") && !s.file.contains("spec") && s.name != "renderWithTheme"));
        assert!(g.file_list.iter().all(|f| !f.contains("__tests__") && !f.contains(".test.") && !f.contains(".spec.")));
    }

    #[test]
    fn imports_link_symbols_to_their_tests() {
        let g = tests_fixture();
        assert_eq!(test_names(&g, "Card"), ["Card.test.tsx"]);
        // through a barrel, from a `__tests__` folder
        assert_eq!(test_names(&g, "Button"), ["__tests__/Button.test.tsx"]);
        // default import
        assert_eq!(test_names(&g, "useCounter"), ["hooks/useCounter.spec.ts"]);
        // `import * as math` covers every export of the file
        assert_eq!(test_names(&g, "add"), ["math.test.ts"]);
        assert_eq!(test_names(&g, "sub"), ["math.test.ts"]);
        // a handler defined inside a tested component counts as tested
        assert_eq!(test_names(&g, "onClick"), ["Card.test.tsx"]);
    }

    #[test]
    fn untested_symbols_are_flagged() {
        let g = tests_fixture();
        let flagged: Vec<&str> = {
            let mut v: Vec<&str> = (0..g.symbols.len()).filter(|&i| g.untested(i)).map(|i| g.symbols[i].name.as_str()).collect();
            v.sort();
            v
        };
        // imported by name only: its neighbours in the same file are not covered
        assert_eq!(flagged, ["CardSkeleton", "IconButton", "Lonely"]);
    }

    #[test]
    fn nothing_is_untested_in_a_project_without_tests() {
        let g = build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/memo"));
        assert!(g.test_files.is_empty());
        assert!((0..g.symbols.len()).all(|i| !g.untested(i)));
    }

    #[test]
    fn complex_react_reducer_test_is_linked() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../complex-react");
        if !root.exists() {
            return;
        }
        let g = build(&root);
        assert!(g.test_files.iter().any(|f| f.ends_with("boardReducer.test.ts")));
        let reducer = idx(&g, "src/features/board/boardReducer.ts", "boardReducer");
        assert!(g.tests_of(reducer).iter().any(|&t| g.test_files[t].ends_with("boardReducer.test.ts")));
    }

    #[test]
    fn a_project_folder_named_like_a_test_is_still_a_project() {
        let base = std::env::temp_dir().join(format!("react-graph-odd-root-{}", std::process::id()));
        for dir in ["my.test.app", "__tests__/proj", "a.spec.b"] {
            let root = base.join(dir);
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::write(root.join("src/App.tsx"), "export function App() { return <div />; }
").unwrap();
            std::fs::write(root.join("src/App.test.tsx"), "import { App } from \"./App\";
test(\"a\", () => { void App; });
").unwrap();
            let g = build(&root);
            assert_eq!((g.files, g.symbols.len(), g.test_files.len()), (1, 1, 1), "{dir}");
            assert_eq!(g.tests_of(0).len(), 1, "{dir}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
