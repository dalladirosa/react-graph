//! Where code talks to the outside: HTTP calls, data-fetching hooks and state stores.
//!
//! Syntax only, by well-known names: `fetch`, `axios.get(...)`, `useQuery` / `useSWR`,
//! `useSelector` / `useDispatch`, `use...Store`, jotai and recoil hooks. Each hit is a `DataUse` on
//! the symbol that makes the call.

use crate::graph::{call_args, callee_name, strip_expr, text};
use tree_sitter::Node;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DataKind {
    /// `fetch`, `axios.get`: an HTTP request
    Http,
    /// `useQuery`, `useSWR`, `useMutation`: a cached server-state hook
    Query,
    /// redux, zustand, jotai, recoil: client state shared between components
    Store,
}

impl DataKind {
    pub fn label(self) -> &'static str {
        match self {
            DataKind::Http => "http",
            DataKind::Query => "query",
            DataKind::Store => "store",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DataUse {
    pub kind: DataKind,
    /// "GET /api/products", "todos", "redux", "useCartStore"
    pub label: String,
    /// 1-based
    pub line: usize,
}

/// A string or template literal as a path: `${id}` becomes `{}`. None when it isn't a literal.
fn literal(n: Node, src: &str) -> Option<String> {
    let n = strip_expr(n);
    match n.kind() {
        "string" => Some(text(n, src).trim_matches(|c| c == '"' || c == '\'').to_string()),
        "template_string" => {
            let mut out = String::new();
            let mut c = n.walk();
            for part in n.named_children(&mut c) {
                match part.kind() {
                    "string_fragment" | "escape_sequence" => out.push_str(text(part, src)),
                    "template_substitution" => out.push_str("{}"),
                    _ => {}
                }
            }
            Some(out)
        }
        _ => None,
    }
}

/// `{ method: "POST" }` => "POST"
fn method_of(opts: Node, src: &str) -> Option<String> {
    let opts = strip_expr(opts);
    if opts.kind() != "object" {
        return None;
    }
    let mut c = opts.walk();
    let found = opts.named_children(&mut c).filter(|p| p.kind() == "pair").find_map(|p| {
        let key = p.child_by_field_name("key")?;
        (text(key, src).trim_matches(|c| c == '"' || c == '\'') == "method")
            .then(|| p.child_by_field_name("value").and_then(|v| literal(v, src)))
            .flatten()
    });
    found.map(|m| m.to_uppercase())
}

/// The first string in a react-query / SWR key: `["todos", id]`, `{ queryKey: ["todos"] }`, `"/api/x"`.
fn key_of(args: &[Node], src: &str) -> Option<String> {
    let first = strip_expr(*args.first()?);
    let key = match first.kind() {
        "object" => {
            let mut c = first.walk();
            let v = first.named_children(&mut c).filter(|p| p.kind() == "pair").find_map(|p| {
                let k = p.child_by_field_name("key")?;
                matches!(text(k, src), "queryKey" | "mutationKey").then(|| p.child_by_field_name("value")).flatten()
            });
            v?
        }
        _ => first,
    };
    let key = strip_expr(key);
    if key.kind() == "array" {
        return key.named_child(0).and_then(|k| literal(k, src));
    }
    literal(key, src)
}

const VERBS: &[&str] = &["get", "post", "put", "patch", "delete", "head"];
const HTTP_CLIENTS: &[&str] = &["axios", "ky", "http", "httpClient", "api", "apiClient"];

/// Data access made by this call, if it is one.
pub fn uses(call: Node, src: &str) -> Option<DataUse> {
    let line = call.start_position().row + 1;
    let name = callee_name(call, src)?;
    let args = call_args(call);
    let f = call.child_by_field_name("function")?;
    let mk = |kind, label: String| Some(DataUse { kind, label, line });
    // `fetch()` / `useQuery()` with nothing to go on is not a call worth listing
    let needs_arg = |n: &str| matches!(n, "fetch" | "axios" | "ky" | "useQuery" | "useInfiniteQuery" | "useSuspenseQuery" | "useSWR" | "useSWRImmutable") || VERBS.contains(&n);
    if args.is_empty() && needs_arg(name) {
        return None;
    }

    // fetch(url, { method }) / axios(url)
    if f.kind() == "identifier" && matches!(name, "fetch" | "axios" | "ky") {
        let url = args.first().and_then(|a| literal(*a, src)).unwrap_or_else(|| "(dynamic url)".into());
        let method = args.get(1).and_then(|o| method_of(*o, src)).unwrap_or_else(|| "GET".into());
        return mk(DataKind::Http, format!("{method} {url}"));
    }
    // axios.get(url) / api.post(url)
    if f.kind() == "member_expression" && VERBS.contains(&name) {
        let obj = f.child_by_field_name("object").filter(|o| o.kind() == "identifier").map(|o| text(o, src))?;
        if HTTP_CLIENTS.contains(&obj) {
            // a verb on a non-HTTP object (`map.get(key)`) has no URL-looking first argument
            let url = args.first().and_then(|a| literal(*a, src));
            if url.as_deref().is_some_and(|u| u.contains('/')) || obj == "axios" {
                let url = url.unwrap_or_else(|| "(dynamic url)".into());
                return mk(DataKind::Http, format!("{} {url}", name.to_uppercase()));
            }
        }
        return None;
    }
    match name {
        "useQuery" | "useInfiniteQuery" | "useSuspenseQuery" | "useSWR" | "useSWRImmutable" => {
            let key = key_of(&args, src).unwrap_or_else(|| "(dynamic key)".into());
            mk(DataKind::Query, key)
        }
        "useMutation" | "useSWRMutation" => {
            let key = key_of(&args, src).unwrap_or_else(|| "mutation".into());
            mk(DataKind::Query, format!("mutation: {key}"))
        }
        "useSelector" | "useDispatch" | "useAppSelector" | "useAppDispatch" | "useStore" => mk(DataKind::Store, "redux".into()),
        "useAtom" | "useAtomValue" | "useSetAtom" => mk(DataKind::Store, "jotai".into()),
        "useRecoilState" | "useRecoilValue" | "useSetRecoilState" => mk(DataKind::Store, "recoil".into()),
        // zustand: `const useCartStore = create(...)`, then `useCartStore(s => s.items)`
        n if f.kind() == "identifier"
            && n.len() > "useStore".len()
            && n.starts_with("use")
            && n.ends_with("Store") =>
        {
            mk(DataKind::Store, n.to_string())
        }
        _ => None,
    }
}
