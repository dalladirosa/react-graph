//! Uncommitted changes: the graph of the project as it is in git's HEAD commit, compared with the
//! graph of the files on disk. Shows which symbols were added, edited or removed, which links
//! appeared or went away, and which problems the change brought in or fixed.
//!
//! The HEAD version is read with the `git` command (`ls-tree` + `cat-file --batch`), written to a
//! temporary folder and parsed with the same `graph::build` as the working tree, so both graphs
//! are built in exactly the same way.

use crate::graph::{self, EdgeKind, Graph, SymKind};
use crate::views::{self, SymKey};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The project as committed in HEAD.
pub struct Head {
    pub sha: String,
    pub graph: Graph,
    /// source of every parsed file, by relative path (forward slashes)
    pub src: HashMap<String, String>,
}

/// Files on disk whose text differs from HEAD (or that HEAD doesn't have), with their text now.
pub type Edited = HashMap<String, String>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Added,
    Edited,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Added => "new",
            Status::Edited => "edited",
        }
    }
}

/// A symbol HEAD has and the working tree doesn't.
#[derive(Clone, Debug)]
pub struct Removed {
    pub name: String,
    pub kind: SymKind,
    pub file: String,
    pub line: usize,
}

/// A link between two symbols that only one side has. `from` / `to` are names; the indexes point
/// into the current graph when the symbol still exists.
#[derive(Clone, Debug)]
pub struct LinkChange {
    pub from: String,
    pub to: String,
    /// files of the two ends (to find a removed end in `Changes::removed`)
    pub from_file: String,
    pub to_file: String,
    pub kind: EdgeKind,
    pub from_idx: Option<usize>,
    pub to_idx: Option<usize>,
    /// true: new in the working tree; false: gone since HEAD
    pub added: bool,
}

/// Something the change made worse (`worse`) or better.
#[derive(Clone, Debug)]
pub struct Issue {
    /// the symbol in the current graph (None when it was removed)
    pub sym: Option<usize>,
    pub text: String,
    pub worse: bool,
}

#[derive(Default)]
pub struct Changes {
    /// HEAD commit, abbreviated
    pub head: String,
    /// symbols of the current graph that are new or edited
    pub status: HashMap<usize, Status>,
    pub removed: Vec<Removed>,
    pub links: Vec<LinkChange>,
    pub issues: Vec<Issue>,
    /// unchanged symbols that directly use a new, edited or removed one: where the change may show
    pub affected: HashSet<usize>,
    /// source files that differ from HEAD (added, edited or deleted)
    pub files: usize,
}

impl Changes {
    pub fn count(&self, s: Status) -> usize {
        self.status.values().filter(|&&v| v == s).count()
    }
    pub fn worse(&self) -> usize {
        self.issues.iter().filter(|i| i.worse).count()
    }
    pub fn better(&self) -> usize {
        self.issues.iter().filter(|i| !i.worse).count()
    }
    pub fn is_empty(&self) -> bool {
        self.status.is_empty() && self.removed.is_empty() && self.links.is_empty() && self.files == 0
    }
    /// Links of `i` that changed (either end).
    pub fn links_of(&self, i: usize) -> impl Iterator<Item = &LinkChange> {
        self.links.iter().filter(move |l| l.from_idx == Some(i) || l.to_idx == Some(i))
    }
    pub fn issues_of(&self, i: usize) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(move |x| x.sym == Some(i))
    }
    /// The removed symbol a link end names, if it is one of the outermost removed ones.
    pub fn removed_named(&self, file: &str, name: &str) -> Option<&Removed> {
        self.removed.iter().find(|r| r.file == file && r.name == name)
    }
}

// ---------------------------------------------------------------- git

/// A path git (and Windows' own tools) understand: no `\\?\` prefix.
fn plain(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}

fn git(root: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(plain(root)).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // no console window flashing up for each call
        c.creation_flags(0x0800_0000);
    }
    c
}

fn git_out(root: &Path, args: &[&str]) -> Option<String> {
    let out = git(root).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The HEAD commit of the repository `root` is in. None outside a repository, without the `git`
/// command, or before the first commit.
pub fn head_sha(root: &Path) -> Option<String> {
    git_out(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).filter(|s| !s.is_empty())
}

/// The repository's git folder (absolute), to watch for commits and checkouts.
pub fn git_dir(root: &Path) -> Option<PathBuf> {
    let d = git_out(root, &["rev-parse", "--absolute-git-dir"])?;
    let p = PathBuf::from(d);
    p.is_dir().then_some(p)
}

/// Does a change to `p` (inside the git folder) move HEAD or what it points at?
pub fn moves_head(p: &Path) -> bool {
    let s = p.to_string_lossy().replace('\\', "/");
    s.ends_with("/HEAD") || s.ends_with("/packed-refs") || s.contains("/refs/heads/")
}

/// Would `build` look at this file (relative to the project folder)?
fn wanted(rel: &str) -> bool {
    let p = Path::new(rel);
    graph::affects_graph(p) || graph::is_test_file(p) || rel == "package.json"
}

/// Read the files of `root` as committed in `sha` that the graph is built from, by relative path.
fn read_tree(root: &Path, sha: &str) -> Result<Vec<(String, String)>, String> {
    // paths come out relative to `root`, limited to it by the "." pathspec
    let out = git(root)
        .args(["ls-tree", "-r", "-z", sha, "--", "."])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err("git ls-tree failed".into());
    }
    let mut want: Vec<(String, String)> = Vec::new(); // (object id, path)
    for rec in out.stdout.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let rec = String::from_utf8_lossy(rec);
        let Some((meta, path)) = rec.split_once('\t') else { continue };
        let mut it = meta.split(' ');
        let (_mode, kind, oid) = (it.next(), it.next(), it.next());
        if kind == Some("blob") && wanted(path) {
            if let Some(oid) = oid {
                want.push((oid.to_string(), path.to_string()));
            }
        }
    }
    if want.is_empty() {
        return Ok(Vec::new());
    }

    let mut child = git(root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("git cat-file: no stdin")?;
    let ids: String = want.iter().map(|(o, _)| format!("{o}\n")).collect();
    // write from another thread: git starts answering before it has read every id
    let feeder = std::thread::spawn(move || {
        let _ = stdin.write_all(ids.as_bytes());
    });
    let mut r = BufReader::new(child.stdout.take().ok_or("git cat-file: no stdout")?);
    let mut files = Vec::with_capacity(want.len());
    let mut header = String::new();
    for (_, path) in &want {
        header.clear();
        if r.read_line(&mut header).map_err(|e| e.to_string())? == 0 {
            break;
        }
        // "<oid> blob <size>", or "<oid> missing"
        let size: Option<usize> = header.trim_end().rsplit(' ').next().and_then(|s| s.parse().ok());
        let Some(size) = size.filter(|_| !header.ends_with("missing\n")) else { continue };
        let mut buf = vec![0u8; size + 1]; // content + "\n"
        r.read_exact(&mut buf).map_err(|e| e.to_string())?;
        buf.truncate(size);
        files.push((path.clone(), String::from_utf8_lossy(&buf).into_owned()));
    }
    let _ = feeder.join();
    let _ = child.wait();
    Ok(files)
}

/// Build the graph of the project as committed in `sha`.
pub fn load_head(root: &Path, sha: &str) -> Result<Head, String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let files = read_tree(root, sha)?;
    let dir = std::env::temp_dir().join(format!("react-graph-head-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    for (rel, text) in &files {
        let p = dir.join(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, text).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let graph = graph::build(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let parsed: HashSet<&String> = graph.file_list.iter().collect();
    let src = files.into_iter().filter(|(f, _)| parsed.contains(f)).collect();
    Ok(Head { sha: sha.to_string(), graph, src })
}

/// Files of the working tree (`files`, relative) whose text differs from `head`.
pub fn edited_files(root: &Path, files: &[String], head: &Head) -> Edited {
    files
        .iter()
        .filter_map(|f| {
            let now = std::fs::read_to_string(root.join(f)).ok()?;
            (head.src.get(f) != Some(&now)).then(|| (f.clone(), now))
        })
        .collect()
}

// ---------------------------------------------------------------- diff

/// Lines `from..=to` (1-based) of `src`, with trailing spaces and line endings ignored.
fn body(src: &str, from: usize, to: usize) -> Vec<&str> {
    src.lines().skip(from.saturating_sub(1)).take(to.saturating_sub(from) + 1).map(str::trim_end).collect()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn cycle_text(g: &Graph, c: usize) -> String {
    let names: Vec<&str> = g.cycles[c].iter().map(|&m| g.symbols[m].name.as_str()).collect();
    match names.first() {
        Some(first) => format!("{} \u{2192} {first}", names.join(" \u{2192} ")),
        None => String::new(),
    }
}

/// Compare the graph of the working tree (`cur`, files that differ from HEAD in `edited`) with `head`.
pub fn diff(head: &Head, cur: &Graph, edited: &Edited) -> Changes {
    let h = &head.graph;
    let hk = views::keys(h);
    let ck = views::keys(cur);
    let h_at: HashMap<&SymKey, usize> = hk.iter().enumerate().map(|(i, k)| (k, i)).collect();
    let c_at: HashMap<&SymKey, usize> = ck.iter().enumerate().map(|(i, k)| (k, i)).collect();
    let cur_files: HashSet<&String> = cur.file_list.iter().collect();
    let deleted = h.file_list.iter().filter(|f| !cur_files.contains(f)).count();

    let mut ch = Changes { head: head.sha.chars().take(7).collect(), files: edited.len() + deleted, ..Changes::default() };

    // symbols: new, or code that differs (a file nobody edited can't hold an edit)
    for (i, k) in ck.iter().enumerate() {
        match h_at.get(k) {
            None => {
                ch.status.insert(i, Status::Added);
            }
            Some(&j) => {
                let Some(now) = edited.get(&k.0) else { continue };
                let (a, b) = (&h.symbols[j], &cur.symbols[i]);
                let then = head.src.get(&k.0).map(String::as_str).unwrap_or("");
                if body(then, a.line, a.end_line) != body(now, b.line, b.end_line) {
                    ch.status.insert(i, Status::Edited);
                }
            }
        }
    }
    // removed: only the outermost (what was defined inside it went with it)
    let gone = |j: usize| !c_at.contains_key(&hk[j]);
    for (j, k) in hk.iter().enumerate() {
        if gone(j) && h.symbols[j].parent.is_none_or(|p| !gone(p)) {
            let s = &h.symbols[j];
            ch.removed.push(Removed { name: s.name.clone(), kind: s.kind, file: k.0.clone(), line: s.line });
            // its warnings (and those of what it defined) went with it
            let w: usize = (0..h.symbols.len()).filter(|&m| gone(m) && h.is_inside(m, j)).map(|m| h.symbols[m].warnings()).sum();
            if w > 0 {
                ch.issues.push(Issue { sym: None, text: format!("{} removed, with its {}", s.name, plural(w, "warning", "warnings")), worse: false });
            }
        }
    }

    // links (nesting is already told by the symbols themselves)
    let set = |g: &Graph, keys: &[SymKey]| -> HashSet<(SymKey, SymKey, EdgeKind)> {
        g.edges
            .iter()
            .filter(|e| e.kind != EdgeKind::Defines)
            .map(|e| (keys[e.from].clone(), keys[e.to].clone(), e.kind))
            .collect()
    };
    let (h_links, c_links) = (set(h, &hk), set(cur, &ck));
    for (added, from_set, other) in [(true, &c_links, &h_links), (false, &h_links, &c_links)] {
        let mut v: Vec<&(SymKey, SymKey, EdgeKind)> = from_set.difference(other).collect();
        v.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        for (f, t, kind) in v {
            ch.links.push(LinkChange {
                from: f.1.clone(),
                to: t.1.clone(),
                from_file: f.0.clone(),
                to_file: t.0.clone(),
                kind: *kind,
                from_idx: c_at.get(f).copied(),
                to_idx: c_at.get(t).copied(),
                added,
            });
        }
    }

    // unchanged code that uses changed code (or lost something it used)
    // (a context link points from the provider to its readers: the provider doesn't use them)
    for e in cur.edges.iter().filter(|e| !matches!(e.kind, EdgeKind::Defines | EdgeKind::Context) && e.from != e.to) {
        if ch.status.contains_key(&e.to) && !ch.status.contains_key(&e.from) {
            ch.affected.insert(e.from);
        }
    }
    for l in ch.links.iter().filter(|l| !l.added) {
        if let Some(f) = l.from_idx.filter(|f| !ch.status.contains_key(f)) {
            ch.affected.insert(f);
        }
    }

    // problems brought in or fixed
    let any_tests = !cur.test_files.is_empty();
    for (i, k) in ck.iter().enumerate() {
        let s = &cur.symbols[i];
        let (w, unused, cycle) = (s.warnings(), cur.is_unused(i), s.cycle);
        let push = |ch: &mut Changes, text: String, worse: bool| ch.issues.push(Issue { sym: Some(i), text, worse });
        match h_at.get(k) {
            None => {
                if w > 0 {
                    push(&mut ch, format!("new, with {}", plural(w, "warning", "warnings")), true);
                }
                if unused && s.parent.is_none() {
                    push(&mut ch, "new, and nothing uses it".into(), true);
                }
                if let Some(c) = cycle {
                    push(&mut ch, format!("new, in a loop: {}", cycle_text(cur, c)), true);
                }
            }
            Some(&j) => {
                let t = &h.symbols[j];
                let was = t.warnings();
                if w > was {
                    push(&mut ch, format!("{} more ({was} \u{2192} {w})", plural(w - was, "warning", "warnings")), true);
                } else if w < was {
                    push(&mut ch, format!("{} fixed ({was} \u{2192} {w})", plural(was - w, "warning", "warnings")), false);
                }
                let was_unused = h.is_unused(j);
                if unused && !was_unused && s.parent.is_none() {
                    push(&mut ch, "no longer used by anything".into(), true);
                } else if !unused && was_unused && s.parent.is_none() {
                    push(&mut ch, "used again".into(), false);
                }
                match (t.cycle, cycle) {
                    (None, Some(c)) => push(&mut ch, format!("now in a loop: {}", cycle_text(cur, c)), true),
                    (Some(_), None) => push(&mut ch, "no longer in a loop".into(), false),
                    _ => {}
                }
            }
        }
        // new code nothing tests (edited code would flag nearly every edit in a lightly tested project)
        if any_tests && !h_at.contains_key(k) && cur.untested(i) && !unused {
            push(&mut ch, "new, and no test imports it".into(), true);
        }
    }
    // worst first, then by place
    ch.issues.sort_by_key(|x| (!x.worse, x.sym.map(|i| (cur.symbols[i].file.clone(), cur.symbols[i].line))));
    ch
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_repo(files: &[(&str, &str)]) -> Option<PathBuf> {
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!("react-graph-git-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (f, text) in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let run = |args: &[&str]| git(&root).args(args).status().is_ok_and(|s| s.success());
        let ok = run(&["init", "-q"])
            && run(&["add", "-A"])
            && run(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"]);
        ok.then_some(root)
    }

    fn changes_for(root: &Path) -> (Changes, Graph) {
        let sha = head_sha(root).expect("head");
        let head = load_head(root, &sha).expect("load head");
        let cur = graph::build(root);
        let edited = edited_files(root, &cur.file_list, &head);
        (diff(&head, &cur, &edited), cur)
    }

    fn idx(g: &Graph, name: &str) -> usize {
        g.symbols.iter().position(|s| s.name == name).unwrap_or_else(|| panic!("no {name}"))
    }

    #[test]
    fn no_changes_right_after_a_commit() {
        let Some(root) = temp_repo(&[
            ("src/App.tsx", "import { Card } from \"./Card\";\nexport function App() { return <Card />; }\n"),
            ("src/Card.tsx", "export function Card() { return <div />; }\n"),
        ]) else {
            eprintln!("no git: skipped");
            return;
        };
        let (ch, _) = changes_for(&root);
        assert!(ch.is_empty(), "{:?} {:?}", ch.status, ch.links);
        assert!(ch.issues.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn added_edited_removed_and_links() {
        let Some(root) = temp_repo(&[
            ("src/App.tsx", "import { Card } from \"./Card\";\nexport function App() { return <Card />; }\n"),
            ("src/Card.tsx", "export function Card() { return <div />; }\n"),
            ("src/Old.tsx", "export function Old() { return <p />; }\nexport function Keep() { return <Old />; }\n"),
        ]) else {
            return;
        };
        std::fs::write(
            root.join("src/App.tsx"),
            "import { Card } from \"./Card\";\nimport { Badge } from \"./Badge\";\nexport function App() { return <div><Card /><Badge /></div>; }\n",
        )
        .unwrap();
        std::fs::write(root.join("src/Badge.tsx"), "export function Badge() { return <span />; }\nexport function Orphan() { return <i />; }\n").unwrap();
        std::fs::write(root.join("src/Old.tsx"), "export function Keep() { return <p />; }\n").unwrap();
        let (ch, g) = changes_for(&root);
        assert_eq!(ch.status.get(&idx(&g, "App")), Some(&Status::Edited));
        assert_eq!(ch.status.get(&idx(&g, "Badge")), Some(&Status::Added));
        assert_eq!(ch.status.get(&idx(&g, "Card")), None, "untouched file");
        assert_eq!(ch.status.get(&idx(&g, "Keep")), Some(&Status::Edited));
        assert_eq!(ch.removed.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Old"]);
        assert!(ch.links.iter().any(|l| l.added && l.from == "App" && l.to == "Badge" && l.kind == EdgeKind::Renders));
        assert!(ch.links.iter().any(|l| !l.added && l.from == "Keep" && l.to == "Old" && l.to_idx.is_none()));
        // a new component nothing renders is a problem the change brought in
        let orphan = idx(&g, "Orphan");
        assert!(ch.issues.iter().any(|x| x.worse && x.sym == Some(orphan) && x.text.contains("nothing uses it")));
        assert_eq!(ch.files, 3);
        // nothing renders App or Keep from outside; Card is rendered by the edited App only
        assert!(ch.affected.is_empty(), "{:?}", ch.affected);
        assert_eq!(ch.removed_named("src/Old.tsx", "Old").map(|r| r.kind), Some(SymKind::Component));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_context_provider_is_not_affected_by_its_readers() {
        let Some(root) = temp_repo(&[
            (
                "src/Theme.tsx",
                "import { createContext } from \"react\";\nimport { Page } from \"./Page\";\nexport const ThemeContext = createContext(\"light\");\nexport function ThemeProvider() { return <ThemeContext.Provider value=\"dark\"><Page /></ThemeContext.Provider>; }\n",
            ),
            (
                "src/Page.tsx",
                "import { useContext } from \"react\";\nimport { ThemeContext } from \"./Theme\";\nexport function Page() { const t = useContext(ThemeContext); return <div>{t}</div>; }\n",
            ),
        ]) else {
            return;
        };
        std::fs::write(
            root.join("src/Page.tsx"),
            "import { useContext } from \"react\";\nimport { ThemeContext } from \"./Theme\";\nexport function Page() { const t = useContext(ThemeContext); return <section>{t}</section>; }\n",
        )
        .unwrap();
        let (ch, g) = changes_for(&root);
        assert_eq!(ch.status.get(&idx(&g, "Page")), Some(&Status::Edited));
        assert!(g.edges.iter().any(|e| e.kind == EdgeKind::Context), "the fixture has a context link");
        // it renders the edited Page, so it is affected through that link, not the context one
        let provider = idx(&g, "ThemeProvider");
        assert!(ch.affected.contains(&provider));
        std::fs::write(
            root.join("src/Theme.tsx"),
            "import { createContext } from \"react\";\nexport const ThemeContext = createContext(\"light\");\nexport function ThemeProvider({ children }: { children: any }) { return <ThemeContext.Provider value=\"dark\">{children}</ThemeContext.Provider>; }\n",
        )
        .unwrap();
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-qam", "provider only"])
            .status();
        std::fs::write(
            root.join("src/Page.tsx"),
            "import { useContext } from \"react\";\nimport { ThemeContext } from \"./Theme\";\nexport function Page() { const t = useContext(ThemeContext); return <article>{t}</article>; }\n",
        )
        .unwrap();
        let (ch, g) = changes_for(&root);
        assert_eq!(ch.status.get(&idx(&g, "Page")), Some(&Status::Edited));
        assert!(!ch.affected.contains(&idx(&g, "ThemeProvider")), "only a context link joins them now");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn users_of_changed_code_are_affected() {
        let Some(root) = temp_repo(&[
            ("src/App.tsx", "import { Card } from \"./Card\";\nexport function App() { return <Card />; }\n"),
            ("src/Card.tsx", "export function Card() { return <div />; }\n"),
            ("src/Page.tsx", "import { App } from \"./App\";\nexport function Page() { return <App />; }\n"),
        ]) else {
            return;
        };
        std::fs::write(root.join("src/Card.tsx"), "export function Card() { return <section />; }\n").unwrap();
        let (ch, g) = changes_for(&root);
        assert_eq!(ch.affected, HashSet::from([idx(&g, "App")]), "App uses the edited Card; Page is two steps away");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn new_and_fixed_cycles() {
        let Some(root) = temp_repo(&[
            ("a.ts", "export function a(n: number) { return b(n); }\nexport function b(n: number) { return a(n); }\n"),
            ("c.ts", "export function c() { return 1; }\nexport function d() { return c(); }\n"),
        ]) else {
            return;
        };
        std::fs::write(root.join("a.ts"), "export function a(n: number) { return n; }\nexport function b(n: number) { return a(n); }\n").unwrap();
        std::fs::write(root.join("c.ts"), "export function c(): number { return d(); }\nexport function d(): number { return c(); }\n").unwrap();
        let (ch, g) = changes_for(&root);
        assert!(ch.issues.iter().any(|x| !x.worse && x.sym == Some(idx(&g, "a")) && x.text.contains("no longer in a loop")));
        assert!(ch.issues.iter().any(|x| x.worse && x.sym == Some(idx(&g, "c")) && x.text.contains("now in a loop")));
        assert!(ch.worse() >= 2 && ch.better() >= 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_subfolder_of_the_repository() {
        let Some(repo) = temp_repo(&[
            ("web/src/App.tsx", "export function App() { return <div />; }\n"),
            ("other/X.tsx", "export function X() { return <div />; }\n"),
        ]) else {
            return;
        };
        let root = repo.join("web");
        std::fs::write(root.join("src/App.tsx"), "export function App() { return <span />; }\n").unwrap();
        let (ch, g) = changes_for(&root);
        assert_eq!(g.symbols.len(), 1, "only the opened folder");
        assert_eq!(ch.status.get(&0), Some(&Status::Edited));
        assert!(ch.removed.is_empty(), "X is outside the opened folder");
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn outside_a_repository() {
        let dir = std::env::temp_dir().join(format!("react-graph-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // the temp folder itself is normally not inside a repository
        if git_out(&dir, &["rev-parse", "--show-toplevel"]).is_none() {
            assert!(head_sha(&dir).is_none());
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
