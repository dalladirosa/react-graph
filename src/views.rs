//! The state of the canvas that is worth keeping between runs, per project: which cards are
//! open, the filters, the selection. Saved to `views.txt` next to the settings, newest project first.
//!
//! Symbols are stored by (file, name, nth symbol with that name in the file) rather than by index,
//! so a view survives rebuilds and edits that don't add or remove a same-named symbol before it.

use crate::graph::Graph;
use crate::prefs;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// How many projects keep a view.
pub const MAX_PROJECTS: usize = 20;
/// Open cards remembered per project (a view with thousands is a view of the whole project anyway).
pub const MAX_OPEN: usize = 400;

/// (file, name, nth symbol of that name in that file)
pub type SymKey = (String, String, usize);

#[derive(Default, Clone, Debug, PartialEq)]
pub struct View {
    /// property filters that are on, by name ("dup", "warn", ...); names this version doesn't know are ignored
    pub filters: Vec<String>,
    /// kind tabs that are on: "component", "hook", "function"
    pub kinds: Vec<String>,
    pub show_functions: bool,
    /// "Dim others" is on by default
    pub dim_others: bool,
    pub focus_file: Option<String>,
    pub selected: Option<SymKey>,
    pub expanded: Vec<SymKey>,
    pub pinned: Vec<SymKey>,
}

impl View {
    /// What a project looks like before anything is done to it.
    pub fn initial() -> Self {
        View { dim_others: true, ..View::default() }
    }
}

/// Keys of every symbol of `g`, by index.
pub fn keys(g: &Graph) -> Vec<SymKey> {
    let mut seen: HashMap<(&str, &str), usize> = HashMap::new();
    g.symbols
        .iter()
        .map(|s| {
            let n = seen.entry((s.file.as_str(), s.name.as_str())).or_default();
            *n += 1;
            (s.file.clone(), s.name.clone(), *n - 1)
        })
        .collect()
}

/// Symbol index of each key (for restoring).
pub fn index(g: &Graph) -> HashMap<SymKey, usize> {
    keys(g).into_iter().enumerate().map(|(i, k)| (k, i)).collect()
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct Store {
    /// most recently saved first
    projects: Vec<(PathBuf, View)>,
}

fn path() -> Option<PathBuf> {
    prefs::dir().map(|d| d.join("views.txt"))
}

fn key_text(k: &SymKey) -> String {
    format!("{}\t{}\t{}", k.0, k.1, k.2)
}

fn parse_key(v: &str) -> Option<SymKey> {
    let mut it = v.splitn(3, '\t');
    let (f, n, i) = (it.next()?, it.next()?, it.next()?);
    if f.is_empty() || n.is_empty() {
        return None;
    }
    Some((f.to_string(), n.to_string(), i.trim().parse().ok()?))
}

impl Store {
    pub fn load() -> Self {
        path().and_then(|f| std::fs::read_to_string(f).ok()).map(|s| Self::parse(&s)).unwrap_or_default()
    }

    /// Best effort, like the settings: a failed write only loses the view for the next run.
    pub fn save(&self) {
        let (Some(f), Some(d)) = (path(), prefs::dir()) else { return };
        if std::fs::create_dir_all(&d).is_ok() {
            let _ = std::fs::write(f, self.serialize());
        }
    }

    pub fn get(&self, project: &Path) -> Option<&View> {
        self.projects.iter().find(|(p, _)| prefs::same(p, project)).map(|(_, v)| v)
    }

    /// Keep `view` for `project`, moving it to the front.
    pub fn put(&mut self, project: &Path, mut view: View) {
        view.expanded.truncate(MAX_OPEN);
        view.pinned.truncate(MAX_OPEN);
        self.projects.retain(|(p, _)| !prefs::same(p, project));
        self.projects.insert(0, (project.to_path_buf(), view));
        self.projects.truncate(MAX_PROJECTS);
    }

    pub fn parse(s: &str) -> Self {
        let mut st = Store::default();
        for line in s.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            if k == "project" {
                if !v.is_empty() {
                    st.projects.push((PathBuf::from(v), View::initial()));
                }
                continue;
            }
            let Some((_, view)) = st.projects.last_mut() else { continue };
            match k {
                "filters" => {
                    view.filters =
                        v.split(',').filter(|f| !f.is_empty() && f.chars().all(|c| c.is_ascii_lowercase())).map(str::to_string).collect()
                }
                "kinds" => {
                    view.kinds = v.split(',').filter(|k| matches!(*k, "component" | "hook" | "function")).map(str::to_string).collect()
                }
                "functions" => view.show_functions = v == "1",
                "dim" => view.dim_others = v != "0",
                "file" if !v.is_empty() => view.focus_file = Some(v.to_string()),
                "selected" => view.selected = parse_key(v),
                "expanded" => view.expanded.extend(parse_key(v)),
                "pinned" => view.pinned.extend(parse_key(v)),
                _ => {}
            }
        }
        st.projects.truncate(MAX_PROJECTS);
        st
    }

    pub fn serialize(&self) -> String {
        let mut s = String::new();
        for (p, v) in &self.projects {
            s.push_str(&format!("project={}\n", p.display()));
            s.push_str(&format!("filters={}\n", v.filters.join(",")));
            s.push_str(&format!("kinds={}\n", v.kinds.join(",")));
            s.push_str(&format!("functions={}\n", u8::from(v.show_functions)));
            s.push_str(&format!("dim={}\n", u8::from(v.dim_others)));
            if let Some(f) = &v.focus_file {
                s.push_str(&format!("file={f}\n"));
            }
            if let Some(k) = &v.selected {
                s.push_str(&format!("selected={}\n", key_text(k)));
            }
            for k in &v.expanded {
                s.push_str(&format!("expanded={}\n", key_text(k)));
            }
            for k in &v.pinned {
                s.push_str(&format!("pinned={}\n", key_text(k)));
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(f: &str, n: &str, i: usize) -> SymKey {
        (f.to_string(), n.to_string(), i)
    }

    fn sample() -> View {
        View {
            filters: vec!["warn".into(), "untested".into()],
            kinds: vec!["component".into()],
            show_functions: true,
            dim_others: false,
            focus_file: Some("src/App.tsx".into()),
            selected: Some(key("src/App.tsx", "App", 0)),
            expanded: vec![key("src/App.tsx", "App", 0), key("src/ui/Button.tsx", "Button", 1)],
            pinned: vec![key("src/x.ts", "helper", 0)],
        }
    }

    #[test]
    fn round_trip() {
        let mut st = Store::default();
        st.put(Path::new("/a"), sample());
        st.put(Path::new("/b"), View::initial());
        assert_eq!(Store::parse(&st.serialize()), st);
        assert_eq!(st.get(Path::new("/a")), Some(&sample()));
        // newest first
        assert_eq!(st.projects[0].0, PathBuf::from("/b"));
    }

    #[test]
    fn put_replaces_and_caps() {
        let mut st = Store::default();
        for i in 0..30 {
            st.put(Path::new(&format!("/p{i}")), View::initial());
        }
        assert_eq!(st.projects.len(), MAX_PROJECTS);
        st.put(Path::new("/p20"), sample());
        assert_eq!(st.projects[0].0, PathBuf::from("/p20"));
        assert_eq!(st.projects.iter().filter(|(p, _)| p == Path::new("/p20")).count(), 1);
        assert!(st.get(Path::new("/p0")).is_none());
        let mut big = View::initial();
        big.expanded = (0..MAX_OPEN + 50).map(|i| key("f", "n", i)).collect();
        st.put(Path::new("/big"), big);
        assert_eq!(st.get(Path::new("/big")).unwrap().expanded.len(), MAX_OPEN);
    }

    #[test]
    fn junk_and_unknown_values_are_ignored() {
        let st = Store::parse(
            "stray=1\nproject=/x\nfilters=warn,BOGUS,,dup\nkinds=hook,thing\nselected=only-one-part\nexpanded=a.ts\tFoo\t2\nexpanded=\t\t\nnoise\nfunctions=1\n",
        );
        let v = st.get(Path::new("/x")).unwrap();
        assert_eq!(v.filters, ["warn", "dup"]);
        assert_eq!(v.kinds, ["hook"]);
        assert_eq!(v.selected, None);
        assert_eq!(v.expanded, [key("a.ts", "Foo", 2)]);
        assert!(v.show_functions && v.dim_others);
    }

    #[test]
    fn keys_number_same_named_symbols_per_file() {
        let g = crate::graph::build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/memo"));
        let ks = keys(&g);
        assert_eq!(ks.len(), g.symbols.len());
        let idx = index(&g);
        assert_eq!(idx.len(), g.symbols.len(), "keys are unique");
        for (i, k) in ks.iter().enumerate() {
            assert_eq!(idx[k], i);
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_match_loosely() {
        let mut st = Store::default();
        st.put(Path::new(r"\\?\E:\Proj"), sample());
        assert!(st.get(Path::new(r"e:\proj\")).is_some());
    }
}
