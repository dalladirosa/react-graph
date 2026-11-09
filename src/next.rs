//! Next.js: which files are routes, and which components run on the server or the client.
//!
//! App Router (`app/`): `page`, `layout`, `loading`, `error`, `not-found`, `template`, `default`
//! and `route` files are entry points nobody imports, so they are marked as such and each is linked
//! from the nearest layout above it. Files are Server Components unless they say `"use client"`
//! (or are rendered by something that does). Pages Router (`pages/`): every page, `_app`,
//! `_document` and `api/` handler is an entry point.

use crate::graph::{Edge, EdgeKind, Hint, SymKind, Symbol};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

/// A `"use client"` / `"use server"` directive at the top of a file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Directive {
    Client,
    Server,
}

/// Where a component runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Env {
    /// a Server Component: rendered on the server, no hooks, no event handlers
    Server,
    /// a Client Component: `implicit` when its file has no directive but a client component renders or calls it
    Client { implicit: bool },
    /// a Server Action (`"use server"` file or function)
    Action,
}

impl Env {
    pub fn label(self) -> &'static str {
        match self {
            Env::Server => "server",
            Env::Client { .. } => "client",
            Env::Action => "action",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryKind {
    Page,
    Layout,
    Template,
    Loading,
    Error,
    NotFound,
    Default,
    /// `route.ts`: an API handler (GET, POST, ...)
    Route,
    /// Pages Router: `_app`
    App,
    /// Pages Router: `_document`
    Document,
    /// Pages Router: a handler under `pages/api`
    Api,
}

impl EntryKind {
    pub fn label(self) -> &'static str {
        match self {
            EntryKind::Page => "page",
            EntryKind::Layout => "layout",
            EntryKind::Template => "template",
            EntryKind::Loading => "loading",
            EntryKind::Error => "error",
            EntryKind::NotFound => "not-found",
            EntryKind::Default => "default",
            EntryKind::Route => "route handler",
            EntryKind::App => "_app",
            EntryKind::Document => "_document",
            EntryKind::Api => "api",
        }
    }
}

/// A file Next.js loads by itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: EntryKind,
    /// "/blog/[slug]"
    pub route: String,
}

/// What `annotate` needs to know about a file.
#[derive(Clone, Debug, Default)]
pub struct FileFacts {
    pub directive: Option<Directive>,
    /// the local name of the default export
    pub default_export: Option<String>,
}

/// Hooks and handlers that only work in the browser.
pub const CLIENT_HOOKS: &[&str] = &[
    "useState", "useReducer", "useEffect", "useLayoutEffect", "useInsertionEffect", "useRef", "useContext",
    "useCallback", "useMemo", "useTransition", "useDeferredValue", "useSyncExternalStore", "useImperativeHandle",
];

/// The folders that hold routes: `app` or `src/app`, `pages` or `src/pages`.
#[derive(Debug, Default, PartialEq)]
pub struct Dirs {
    pub app: Option<String>,
    pub pages: Option<String>,
}

fn stem(file: &str) -> &str {
    let name = file.rsplit('/').next().unwrap_or(file);
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

fn dir_of(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(d, _)| d)
}

/// Is this a Next.js project, and where are its routes?
pub fn detect(root: &Path, files: &[String]) -> Dirs {
    let next_dep = std::fs::read_to_string(root.join("package.json")).is_ok_and(|s| s.contains("\"next\""));
    let has = |prefix: &str, stems: &[&str]| files.iter().any(|f| f.strip_prefix(prefix).is_some_and(|r| stems.contains(&stem(r))));
    let app = ["app", "src/app"].into_iter().find(|d| has(&format!("{d}/"), &["page", "layout"]) || (next_dep && has(&format!("{d}/"), &["route"])));
    let pages = ["pages", "src/pages"]
        .into_iter()
        .find(|d| has(&format!("{d}/"), &["_app", "_document"]) || (next_dep && files.iter().any(|f| f.starts_with(&format!("{d}/")))));
    Dirs { app: app.map(String::from), pages: pages.map(String::from) }
}

/// `app/(shop)/blog/[slug]/page.tsx` => `/blog/[slug]`: route groups, parallel slots and
/// intercepting markers don't appear in the URL.
pub fn app_route(app: &str, file: &str) -> String {
    let rel = file.strip_prefix(app).unwrap_or(file).trim_start_matches('/');
    let segs: Vec<&str> = dir_of(rel)
        .split('/')
        .filter(|s| !s.is_empty() && !(s.starts_with('(') && s.ends_with(')')) && !s.starts_with('@'))
        .collect();
    format!("/{}", segs.join("/"))
}

/// `pages/posts/[id].tsx` => `/posts/[id]`, `pages/index.tsx` => `/`.
pub fn pages_route(pages: &str, file: &str) -> String {
    let rel = file.strip_prefix(pages).unwrap_or(file).trim_start_matches('/');
    let rel = rel.rsplit_once('.').map_or(rel, |(s, _)| s);
    let rel = rel.strip_suffix("/index").unwrap_or(if rel == "index" { "" } else { rel });
    format!("/{rel}")
}

const VERBS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// Mark entries, environments and hints, and link layouts to what they wrap.
pub fn annotate(root: &Path, symbols: &mut [Symbol], edges: &mut Vec<Edge>, files: &[String], facts: &HashMap<&str, FileFacts>) {
    let dirs = detect(root, files);
    if dirs.app.is_none() && dirs.pages.is_none() {
        return;
    }
    let n = symbols.len();
    let top = |symbols: &[Symbol], mut i: usize| {
        while let Some(p) = symbols[i].parent {
            i = p;
        }
        i
    };

    // ---- entries
    let mut by_file: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, s) in symbols.iter().enumerate().filter(|(_, s)| s.parent.is_none()) {
        by_file.entry(s.file.as_str()).or_default().push(i);
    }
    let default_symbol = |symbols: &[Symbol], file: &str| -> Option<usize> {
        let tops = by_file.get(file)?;
        let name = facts.get(file).and_then(|f| f.default_export.as_deref());
        match name {
            Some(nm) => tops.iter().copied().find(|&i| symbols[i].name == nm),
            None => (tops.len() == 1).then(|| tops[0]),
        }
    };
    let mut entries: Vec<(usize, Entry)> = Vec::new();
    for file in files {
        if let Some(app) = dirs.app.as_deref().filter(|a| file.starts_with(&format!("{a}/"))) {
            let kind = match stem(file) {
                "page" => EntryKind::Page,
                "layout" => EntryKind::Layout,
                "template" => EntryKind::Template,
                "loading" => EntryKind::Loading,
                "error" | "global-error" => EntryKind::Error,
                "not-found" => EntryKind::NotFound,
                "default" => EntryKind::Default,
                "route" => EntryKind::Route,
                _ => continue,
            };
            let route = app_route(app, file);
            if kind == EntryKind::Route {
                for &i in by_file.get(file.as_str()).into_iter().flatten() {
                    if VERBS.contains(&symbols[i].name.as_str()) {
                        entries.push((i, Entry { kind, route: route.clone() }));
                    }
                }
            } else if let Some(i) = default_symbol(symbols, file) {
                entries.push((i, Entry { kind, route }));
            }
        } else if let Some(pages) = dirs.pages.as_deref().filter(|p| file.starts_with(&format!("{p}/"))) {
            let rel = file.strip_prefix(pages).unwrap_or(file).trim_start_matches('/');
            let kind = match (stem(file), rel.starts_with("api/")) {
                ("_app", _) => EntryKind::App,
                ("_document", _) => EntryKind::Document,
                (_, true) => EntryKind::Api,
                _ => EntryKind::Page,
            };
            if let Some(i) = default_symbol(symbols, file) {
                let route = match kind {
                    EntryKind::App | EntryKind::Document => format!("({})", stem(file)),
                    _ => pages_route(pages, file),
                };
                entries.push((i, Entry { kind, route }));
            }
        }
    }
    for (i, e) in &entries {
        symbols[*i].entry = Some(e.clone());
    }

    if let Some(app) = dirs.app.as_deref() {
        // ---- server / client
        let dir_of_file = |f: &str| facts.get(f).and_then(|x| x.directive);
        let mut env: Vec<Env> = (0..n)
            .map(|i| {
                let s = &symbols[i];
                match dir_of_file(&s.file) {
                    Some(Directive::Client) => Env::Client { implicit: false },
                    Some(Directive::Server) => Env::Action,
                    None if s.server_action => Env::Action,
                    None => Env::Server,
                }
            })
            .collect();
        // whatever a client component renders or calls ends up in the browser too
        let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
        for e in edges.iter().filter(|e| matches!(e.kind, EdgeKind::Renders | EdgeKind::Calls | EdgeKind::Handler)) {
            out[e.from].push(e.to);
        }
        let mut all_by_file: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, s) in symbols.iter().enumerate() {
            all_by_file.entry(s.file.as_str()).or_default().push(i);
        }
        let mut queue: VecDeque<usize> = (0..n).filter(|&i| matches!(env[i], Env::Client { .. })).collect();
        while let Some(i) = queue.pop_front() {
            for &t in &out[i] {
                let tt = top(symbols, t);
                if env[tt] == Env::Server {
                    // the whole file comes along
                    for &j in &all_by_file[symbols[tt].file.as_str()] {
                        if env[j] == Env::Server {
                            env[j] = Env::Client { implicit: true };
                            queue.push_back(j);
                        }
                    }
                }
            }
        }
        // only code the App Router can reach is a Server or Client Component: files in `pages/`, dead code
        // and helpers nobody renders from `app/` say nothing about where they run (unless they say so themselves)
        let mut in_app = vec![false; n];
        let mut reach: Vec<Vec<usize>> = vec![Vec::new(); n];
        for e in edges.iter().filter(|e| matches!(e.kind, EdgeKind::Renders | EdgeKind::Calls | EdgeKind::Handler | EdgeKind::Defines)) {
            reach[e.from].push(e.to);
        }
        let mut queue: VecDeque<usize> = entries
            .iter()
            .filter(|(i, _)| symbols[*i].file.starts_with(&format!("{app}/")))
            .map(|(i, _)| *i)
            .collect();
        for &i in &queue {
            in_app[i] = true;
        }
        while let Some(i) = queue.pop_front() {
            for &t in &reach[i] {
                if !std::mem::replace(&mut in_app[t], true) {
                    queue.push_back(t);
                }
            }
        }
        let client_only: HashSet<&str> = CLIENT_HOOKS.iter().copied().collect();
        for i in 0..n {
            let explicit = dir_of_file(&symbols[i].file).is_some() || symbols[i].server_action;
            if !in_app[i] && !explicit {
                continue;
            }
            symbols[i].env = Some(env[i]);
            // interactive code in a Server Component
            let s = &symbols[i];
            if env[i] == Env::Server && s.kind == SymKind::Component && s.parent.is_none() && !s.client_uses.is_empty() {
                let (what, _) = &s.client_uses[0];
                let hook = client_only.contains(what.as_str());
                let text = format!(
                    "`{what}` is {} here, but `{}` is a Server Component: its file has no \"use client\" and no client component renders it. Add \"use client\" at the top of the file, or move the interactive part into a client component.",
                    if hook { "called" } else { "used" },
                    s.name
                );
                symbols[i].next_hints.push(Hint { warn: true, text });
            }
        }

        // ---- layouts wrap the pages and layouts below them
        let layouts: Vec<(usize, &str)> = entries
            .iter()
            .filter(|(_, e)| e.kind == EntryKind::Layout)
            .map(|(i, _)| (*i, dir_of(&symbols[*i].file)))
            .collect();
        let mut add: Vec<(usize, usize)> = Vec::new();
        for (i, e) in entries.iter().filter(|(_, e)| !matches!(e.kind, EntryKind::Route)) {
            let file = symbols[*i].file.as_str();
            if !file.starts_with(&format!("{app}/")) {
                continue;
            }
            let d = dir_of(file);
            let inside = |ld: &str| d == ld || d.starts_with(&format!("{ld}/"));
            let parent = layouts
                .iter()
                .filter(|(l, ld)| l != i && inside(ld) && (e.kind != EntryKind::Layout || *ld != d))
                .max_by_key(|(_, ld)| ld.len())
                .map(|(l, _)| *l);
            if let Some(p) = parent {
                add.push((p, *i));
            }
        }
        let have: HashSet<(usize, usize)> = edges.iter().filter(|e| e.kind == EdgeKind::Renders).map(|e| (e.from, e.to)).collect();
        for (p, c) in add {
            if !have.contains(&(p, c)) {
                edges.push(Edge { from: p, to: c, kind: EdgeKind::Renders, line: symbols[p].line, site: p, ctx: None });
            }
        }
    }
}
