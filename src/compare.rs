//! Side-by-side comparison of two similar symbols: aligned rows, and the tokens that differ
//! mapped onto the lines the code panel shows.

use crate::graph::{self, Graph};
use crate::similar::{self, DiffKind, Row, Side};
use crate::source::{self, OffsetMap, SourceFile};
use gpui::HighlightStyle;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;
use tree_sitter::{Node, Parser};

/// A differing token on one displayed line.
#[derive(Clone, Debug)]
pub struct Mark {
    /// byte range within the displayed line
    pub range: Range<usize>,
    /// index into `Compare::pairs`
    pub pair: usize,
}

/// Distinct `a → b` replacements, with how often each occurs.
#[derive(Clone, Debug)]
pub struct Pair {
    pub a: String,
    pub b: String,
    pub kind: DiffKind,
    pub count: usize,
    /// row (index into `Compare::rows`) of the first occurrence
    pub row: usize,
}

pub struct Compare {
    pub a: usize,
    pub b: usize,
    /// token-for-token alignment (structurally identical); otherwise lines were matched
    pub exact: bool,
    pub src_a: Rc<SourceFile>,
    pub src_b: Rc<SourceFile>,
    pub rows: Vec<Row>,
    /// differing tokens per displayed line (0-based), sorted, non-overlapping
    pub marks_a: HashMap<usize, Vec<Mark>>,
    pub marks_b: HashMap<usize, Vec<Mark>>,
    /// parameter-worthy differences first, local renames last
    pub pairs: Vec<Pair>,
}

impl Compare {
    pub fn changed_lines(&self) -> (usize, usize) {
        let ch = self.rows.iter().filter(|r| r.changed);
        (ch.clone().filter(|r| r.a.is_some()).count(), ch.filter(|r| r.b.is_some()).count())
    }
}

/// One side, loaded: raw text (what the parser saw), its display version and offset mapping.
struct Loaded {
    raw: String,
    map: OffsetMap,
    /// byte offset of each line start in the normalized text
    starts: Vec<usize>,
    file: Rc<SourceFile>,
    rows: (usize, usize),
    func: (usize, usize),
}

fn load(g: &Graph, root: &Path, i: usize) -> Result<Loaded, String> {
    let s = &g.symbols[i];
    let func = s.func.ok_or_else(|| format!("{} has no function body to compare", s.name))?;
    let path = root.join(&s.file);
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("Can't read {}: {e}", path.display()))?;
    let norm = source::normalize(&raw);
    let starts = std::iter::once(0).chain(norm.match_indices('\n').map(|(i, _)| i + 1)).collect();
    Ok(Loaded {
        map: OffsetMap::new(&raw),
        file: Rc::new(source::from_text(&norm)),
        starts,
        raw,
        rows: (s.line - 1, s.end_line - 1),
        func,
    })
}

/// The node spanning exactly `range` (the function the symbol was fingerprinted from).
fn node_at<'t>(root: Node<'t>, range: (usize, usize)) -> Option<Node<'t>> {
    let mut n = root.descendant_for_byte_range(range.0, range.1)?;
    while (n.start_byte(), n.end_byte()) != range {
        n = n.parent()?;
        if n.start_byte() < range.0 || n.end_byte() > range.1 {
            return None;
        }
    }
    Some(n)
}

impl Loaded {
    /// Raw byte range -> (displayed line, byte range within it), split at line ends.
    fn pieces(&self, r: &Range<usize>) -> Vec<(usize, Range<usize>)> {
        let (s, e) = (self.map.map(r.start), self.map.map(r.end));
        let mut out = Vec::new();
        let mut line = self.starts.partition_point(|&p| p <= s) - 1;
        let mut pos = s;
        while pos < e && line < self.file.lines.len() {
            let ls = self.starts[line];
            let le = ls + self.file.lines[line].len();
            let (a, b) = (pos.max(ls), e.min(le));
            if b > a {
                out.push((line, a - ls..b - ls));
            }
            line += 1;
            pos = self.starts.get(line).copied().unwrap_or(e);
        }
        out
    }

    /// How a token reads in the differences list.
    fn label(&self, r: &Range<usize>, kind: DiffKind, node_kind: Option<&str>) -> String {
        let t = &self.raw[r.clone()];
        let mut s = match (kind, node_kind) {
            (DiffKind::Code, _) => t.lines().next().unwrap_or("").trim().to_string() + " …",
            (_, Some("string_fragment")) => format!("\"{t}\""),
            _ => t.split_whitespace().collect::<Vec<_>>().join(" "),
        };
        if s.chars().count() > 40 {
            s = s.chars().take(39).collect::<String>() + "…";
        }
        s
    }
}

/// Line up symbols `a` and `b` (two entries of each other's `similar` list).
pub fn build(g: &Graph, root: &Path, a: usize, b: usize) -> Result<Compare, String> {
    let (la, lb) = (load(g, root, a)?, load(g, root, b)?);
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_typescript::LANGUAGE_TSX.into()).map_err(|e| e.to_string())?;
    let ta = parser.parse(&la.raw, None).ok_or("parse failed")?;
    let tb = parser.parse(&lb.raw, None).ok_or("parse failed")?;
    let changed = || "The file changed on disk. Reload the folder to compare again.".to_string();
    let fa = node_at(ta.root_node(), la.func).ok_or_else(changed)?;
    let fb = node_at(tb.root_node(), lb.func).ok_or_else(changed)?;
    let al = similar::align(
        Side { func: fa, src: &la.raw, rows: la.rows },
        Side { func: fb, src: &lb.raw, rows: lb.rows },
        &graph::FN_ROOTS,
        graph::defines_symbol,
    );

    // first row showing each line of a / b
    let mut row_of_a = HashMap::new();
    let mut row_of_b = HashMap::new();
    for (i, r) in al.rows.iter().enumerate() {
        if let Some(x) = r.a {
            row_of_a.entry(x).or_insert(i);
        }
        if let Some(x) = r.b {
            row_of_b.entry(x).or_insert(i);
        }
    }

    fn kind_at(t: &tree_sitter::Tree, r: &Range<usize>) -> Option<String> {
        t.root_node().descendant_for_byte_range(r.start, r.end).map(|n| n.kind().to_string())
    }
    let mut pairs: Vec<Pair> = Vec::new();
    let mut index: HashMap<(DiffKind, String, String), usize> = HashMap::new();
    let mut marks_a: HashMap<usize, Vec<Mark>> = HashMap::new();
    let mut marks_b: HashMap<usize, Vec<Mark>> = HashMap::new();
    for d in &al.diffs {
        let ka = kind_at(&ta, &d.a);
        let kb = kind_at(&tb, &d.b);
        let key = (d.kind, la.label(&d.a, d.kind, ka.as_deref()), lb.label(&d.b, d.kind, kb.as_deref()));
        let (pa, pb) = (la.pieces(&d.a), lb.pieces(&d.b));
        let row = pa
            .first()
            .and_then(|(l, _)| row_of_a.get(l))
            .or_else(|| pb.first().and_then(|(l, _)| row_of_b.get(l)))
            .copied()
            .unwrap_or(0);
        let p = *index.entry(key.clone()).or_insert_with(|| {
            pairs.push(Pair { a: key.1.clone(), b: key.2.clone(), kind: key.0, count: 0, row });
            pairs.len() - 1
        });
        pairs[p].count += 1;
        for (line, range) in pa {
            marks_a.entry(line).or_default().push(Mark { range, pair: p });
        }
        for (line, range) in pb {
            marks_b.entry(line).or_default().push(Mark { range, pair: p });
        }
    }
    for m in marks_a.values_mut().chain(marks_b.values_mut()) {
        m.sort_by_key(|k| k.range.start);
    }
    // parameter-worthy kinds first; keep source order within a kind. Remap mark indices to match.
    let mut order: Vec<usize> = (0..pairs.len()).collect();
    order.sort_by_key(|&i| (pairs[i].kind == DiffKind::Local, pairs[i].kind == DiffKind::Code, pairs[i].row));
    let mut new_ix = vec![0; pairs.len()];
    for (n, &old) in order.iter().enumerate() {
        new_ix[old] = n;
    }
    let pairs = order.iter().map(|&i| pairs[i].clone()).collect();
    for m in marks_a.values_mut().chain(marks_b.values_mut()).flatten() {
        m.pair = new_ix[m.pair];
    }

    Ok(Compare {
        a,
        b,
        exact: al.exact,
        src_a: la.file,
        src_b: lb.file,
        rows: al.rows,
        marks_a,
        marks_b,
        pairs,
    })
}

/// Lay `marks` (sorted, non-overlapping) over `base` syntax highlighting, splitting ranges where
/// they cross; both inputs are byte ranges on the same line.
pub fn overlay(
    base: &[(Range<usize>, HighlightStyle)],
    marks: &[(Range<usize>, HighlightStyle)],
) -> Vec<(Range<usize>, HighlightStyle)> {
    if marks.is_empty() {
        return base.to_vec();
    }
    let mut cuts: Vec<usize> = base.iter().chain(marks).flat_map(|(r, _)| [r.start, r.end]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let at = |list: &[(Range<usize>, HighlightStyle)], p: usize| {
        list.iter().find(|(r, _)| r.start <= p && p < r.end).map(|(_, s)| *s)
    };
    cuts.windows(2)
        .filter_map(|w| {
            let style = match (at(base, w[0]), at(marks, w[0])) {
                (None, None) => return None,
                (Some(s), None) | (None, Some(s)) => s,
                (Some(s), Some(m)) => s.highlight(m),
            };
            Some((w[0]..w[1], style))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Graph, std::path::PathBuf) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/similar");
        (graph::build(&root), root)
    }

    fn sym(g: &Graph, file: &str, name: &str) -> usize {
        g.symbols.iter().position(|s| s.file == file && s.name == name).expect(name)
    }

    fn pairs(c: &Compare, kind: DiffKind) -> Vec<(String, String, usize)> {
        c.pairs.iter().filter(|p| p.kind == kind).map(|p| (p.a.clone(), p.b.clone(), p.count)).collect()
    }

    #[test]
    fn identical_copy_lists_what_to_parameterize() {
        let (g, root) = fixture();
        let (a, b) = (sym(&g, "ProductList.tsx", "ProductList"), sym(&g, "OrderList.tsx", "OrderList"));
        let c = build(&g, &root, a, b).unwrap();
        assert!(c.exact);
        assert!(c.rows.iter().all(|r| !r.changed && r.a.is_some() && r.b.is_some()), "{:?}", c.rows);
        // props, state and callbacks are renames; data fields, class names and texts are real differences
        let names = pairs(&c, DiffKind::Name);
        assert!(names.contains(&("name".into(), "ref".into(), 2)), "{names:?}");
        assert!(names.contains(&("price".into(), "total".into(), 1)), "{names:?}");
        let lits = pairs(&c, DiffKind::Literal);
        assert!(lits.contains(&("\"list\"".into(), "\"orders\"".into(), 1)), "{lits:?}");
        assert!(lits.contains(&("\"Search products\"".into(), "\"Search orders\"".into(), 1)), "{lits:?}");
        assert!(lits.contains(&("Clear".into(), "Reset".into(), 1)), "{lits:?}");
        let locals = pairs(&c, DiffKind::Local);
        for (x, y) in [("query", "filter"), ("items", "orders"), ("visible", "shown"), ("p", "o")] {
            assert!(locals.iter().any(|l| l.0 == x && l.1 == y), "{x}: {locals:?}");
        }
        // the nested `close` / `reset` handler has the same structure: its name is a rename
        assert!(locals.iter().any(|l| l.0 == "close" && l.1 == "reset"), "{locals:?}");
        assert!(pairs(&c, DiffKind::Code).is_empty());
        // local renames come last
        assert!(c.pairs.iter().skip_while(|p| p.kind != DiffKind::Local).all(|p| p.kind == DiffKind::Local));
        // the marks point at the right text on each side
        let (line, m) = c.marks_a.iter().find(|(_, v)| v.iter().any(|m| c.pairs[m.pair].a == "price")).unwrap();
        let m = m.iter().find(|m| c.pairs[m.pair].a == "price").unwrap();
        assert_eq!(&c.src_a.lines[*line][m.range.clone()], "price");
    }

    #[test]
    fn function_and_arrow_copies_align() {
        let (g, root) = fixture();
        let (a, b) = (sym(&g, "ProductList.tsx", "formatPrice"), sym(&g, "OrderList.tsx", "formatTotal"));
        let c = build(&g, &root, a, b).unwrap();
        assert!(c.exact);
        assert!(pairs(&c, DiffKind::Name).is_empty() && pairs(&c, DiffKind::Literal).is_empty(), "{:?}", c.pairs);
        let locals = pairs(&c, DiffKind::Local);
        assert!(locals.contains(&("value".into(), "amount".into(), 2)), "{locals:?}");
        assert!(locals.contains(&("currency".into(), "code".into(), 3)), "{locals:?}");
    }

    #[test]
    fn near_copy_shows_extra_lines() {
        let (g, root) = fixture();
        let (a, b) = (sym(&g, "ProductList.tsx", "ProductList"), sym(&g, "UserList.tsx", "UserList"));
        let c = build(&g, &root, a, b).unwrap();
        assert!(!c.exact);
        // UserList has one extra line (the counter), nothing is missing from it
        assert_eq!(c.changed_lines(), (0, 1));
        let extra = c.rows.iter().find(|r| r.changed).unwrap();
        assert!(c.src_b.lines[extra.b.unwrap()].contains("className=\"count\""));
        assert!(pairs(&c, DiffKind::Name).contains(&("price".into(), "email".into(), 1)), "{:?}", c.pairs);
    }

    #[test]
    fn every_similar_pair_in_complex_react_compares() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../complex-react");
        if !root.exists() {
            return;
        }
        let g = graph::build(&root);
        let mut n = 0;
        for (a, s) in g.symbols.iter().enumerate() {
            for m in &s.similar {
                let c = build(&g, &root, a, m.other).unwrap_or_else(|e| panic!("{}: {e}", s.name));
                for (src, marks) in [(&c.src_a, &c.marks_a), (&c.src_b, &c.marks_b)] {
                    for (&l, ms) in marks {
                        let line = &src.lines[l];
                        for k in ms {
                            assert!(k.range.end <= line.len() && line.is_char_boundary(k.range.start));
                            assert!(line.is_char_boundary(k.range.end) && k.pair < c.pairs.len());
                        }
                    }
                }
                // every line of both symbols is shown exactly once, in order
                let side = |f: fn(&Row) -> Option<usize>| c.rows.iter().filter_map(f).collect::<Vec<_>>();
                let span = |i: usize| (g.symbols[i].line - 1..g.symbols[i].end_line).collect::<Vec<_>>();
                assert_eq!(side(|r| r.a), span(a), "{}", s.name);
                assert_eq!(side(|r| r.b), span(m.other), "{}", s.name);
                n += 1;
            }
        }
        println!("{n} similar pairs compared");
    }

    #[test]
    fn offsets_follow_tabs_and_crlf() {
        let raw = "a\r\n\tb\tc\r\nd";
        let m = OffsetMap::new(raw);
        let norm = source::normalize(raw);
        for (i, ch) in raw.char_indices().filter(|(_, c)| c.is_alphabetic()) {
            assert_eq!(norm[m.map(i)..].chars().next(), Some(ch));
        }
    }

    #[test]
    fn overlay_splits_ranges() {
        let s = |c: u32| HighlightStyle { color: Some(gpui::rgb(c).into()), ..Default::default() };
        let bg = HighlightStyle { background_color: Some(gpui::rgb(0xff0000).into()), ..Default::default() };
        let out = overlay(&[(0..10, s(1))], &[(3..5, bg)]);
        let ranges: Vec<_> = out.iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(ranges, [0..3, 3..5, 5..10]);
        assert!(out[1].1.background_color.is_some() && out[1].1.color.is_some());
    }
}
