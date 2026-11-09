//! Structural fingerprints of function bodies, for spotting duplicated or near-duplicated code.
//!
//! A body is flattened into its sequence of syntax-node kinds (pre-order). Names and literal
//! values are not part of a node's kind, so `filter.priorities.includes(p)` and
//! `opts.tags.includes(t)` produce the same sequence. Two bodies are compared by the Jaccard
//! similarity of their k-shingles (overlapping runs of `K` kinds).

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use tree_sitter::Node;

/// Bodies smaller than this many syntax nodes are too small to be worth flagging
/// (`() => setOpen(false)` is about a dozen).
pub const MIN_NODES: usize = 50;
/// Shingle-set similarity needed to report two bodies as similar.
pub const MIN_SCORE: f32 = 0.8;
/// Shingle length, in syntax nodes.
const K: usize = 6;
/// Stands in for a nested function that is its own symbol (it is fingerprinted separately).
const PLACEHOLDER: u16 = u16::MAX;
/// Type syntax says little about duplicated logic; leave it out.
const IGNORED: &[&str] = &["comment", "type_annotation", "type_arguments", "type_parameters"];

#[derive(Clone, Debug, Default)]
pub struct Shape {
    /// syntax nodes in the body
    pub nodes: usize,
    /// hash of the whole node sequence: equal hashes = same structure, only names/literals differ
    hash: u64,
    /// sorted, deduplicated shingle hashes
    shingles: Vec<u64>,
}

impl Shape {
    pub fn len(&self) -> usize {
        self.shingles.len()
    }
}

fn hash_of<T: Hash + ?Sized>(v: &T) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// Fingerprint the subtrees `roots` (a function's parameters and body). `nested(n)` says whether
/// `n` defines a separate symbol; such subtrees count as a single placeholder node.
pub fn shape<'t>(roots: &[Node<'t>], nested: impl Fn(Node<'t>) -> bool) -> Shape {
    let mut toks: Vec<u16> = Vec::new();
    let mut stack: Vec<Node> = roots.iter().rev().copied().collect();
    while let Some(n) = stack.pop() {
        if n.is_extra() || IGNORED.contains(&n.kind()) {
            continue;
        }
        if nested(n) {
            toks.push(PLACEHOLDER);
            continue;
        }
        toks.push(n.kind_id());
        let mut c = n.walk();
        let kids: Vec<Node> = n.children(&mut c).collect();
        stack.extend(kids.into_iter().rev());
    }
    let mut shingles: Vec<u64> = if toks.len() < K {
        vec![hash_of(&toks[..])]
    } else {
        toks.windows(K).map(hash_of).collect()
    };
    shingles.sort_unstable();
    shingles.dedup();
    Shape { nodes: toks.len(), hash: hash_of(&toks[..]), shingles }
}

/// `Some((score, exact))` when the two bodies are at least `MIN_SCORE` similar.
/// `exact` = identical structure (a copy with renamed identifiers / changed literals).
pub fn compare(a: &Shape, b: &Shape) -> Option<(f32, bool)> {
    if a.nodes < MIN_NODES || b.nodes < MIN_NODES {
        return None;
    }
    if a.hash == b.hash && a.nodes == b.nodes {
        return Some((1.0, true));
    }
    let (mut i, mut j, mut common) = (0, 0, 0usize);
    while i < a.shingles.len() && j < b.shingles.len() {
        match a.shingles[i].cmp(&b.shingles[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                common += 1;
                i += 1;
                j += 1;
            }
        }
    }
    let union = a.len() + b.len() - common;
    let score = common as f32 / union.max(1) as f32;
    (score >= MIN_SCORE).then_some((score, false))
}

// ---------------------------------------------------------------- side-by-side alignment

/// What a differing token is, from the point of view of extracting shared code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DiffKind {
    /// a property, JSX tag, called function or other name from outside: would become a parameter
    Name,
    /// a string, number or JSX text: would become a parameter
    Literal,
    /// a nested function whose structure differs
    Code,
    /// a name declared inside the function on both sides: a plain rename, nothing to parameterize
    Local,
}

/// A token that differs between the two bodies (byte ranges into each source).
#[derive(Clone, Debug)]
pub struct TokenDiff {
    pub a: Range<usize>,
    pub b: Range<usize>,
    pub kind: DiffKind,
}

/// One row of the side-by-side view: 0-based lines, `None` = padding on that side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub a: Option<usize>,
    pub b: Option<usize>,
    /// the lines don't correspond (code only on one side, or a different line in its place)
    pub changed: bool,
}

pub struct Alignment {
    /// token-for-token alignment of the whole body (otherwise lines were matched by structure)
    pub exact: bool,
    pub diffs: Vec<TokenDiff>,
    pub rows: Vec<Row>,
}

/// One function to align: its node, its file's text, and the lines (0-based, inclusive) to show.
#[derive(Clone, Copy)]
pub struct Side<'t, 's> {
    pub func: Node<'t>,
    pub src: &'s str,
    pub rows: (usize, usize),
}

/// LCS table cells above this fall back to showing the lines unmatched.
const MAX_LCS: usize = 4_000_000;

fn kids<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    let mut c = n.walk();
    n.children(&mut c).filter(|k| !k.is_extra() && !IGNORED.contains(&k.kind())).collect()
}

fn roots_of<'t>(func: Node<'t>, fields: &[&str]) -> Vec<Option<Node<'t>>> {
    fields.iter().map(|f| func.child_by_field_name(f)).collect()
}

/// Is `n` the name being declared (variable, parameter, destructured binding, function name)?
fn is_binding(n: Node) -> bool {
    let Some(p) = n.parent() else { return false };
    let is_field = |f: &str| p.child_by_field_name(f).is_some_and(|c| c.id() == n.id());
    match (n.kind(), p.kind()) {
        ("shorthand_property_identifier_pattern", _) => true,
        ("identifier", "variable_declarator") => is_field("name"),
        ("identifier", "required_parameter" | "optional_parameter") => is_field("pattern"),
        ("identifier", "arrow_function") => is_field("parameter"),
        ("identifier", "function_declaration" | "function_expression" | "generator_function_declaration") => {
            is_field("name")
        }
        ("identifier", "array_pattern" | "rest_pattern" | "formal_parameters") => true,
        ("identifier", "pair_pattern") => is_field("value"),
        ("identifier", "assignment_pattern" | "object_assignment_pattern") => is_field("left"),
        ("identifier", "catch_clause") => is_field("parameter"),
        _ => false,
    }
}

/// Every name declared anywhere inside `func` (its parameters included).
fn declared<'s>(func: Node, src: &'s str) -> HashSet<&'s str> {
    let mut out = HashSet::new();
    let mut stack = vec![func];
    while let Some(n) = stack.pop() {
        if n.child_count() == 0 {
            if is_binding(n) {
                out.insert(&src[n.byte_range()]);
            }
            continue;
        }
        let mut c = n.walk();
        stack.extend(n.children(&mut c));
    }
    out
}

/// Text that matters when comparing two leaves (JSX text: whitespace collapsed).
fn leaf_text<'s>(n: Node, src: &'s str) -> std::borrow::Cow<'s, str> {
    let t = &src[n.byte_range()];
    if n.kind() == "jsx_text" { t.split_whitespace().collect::<Vec<_>>().join(" ").into() } else { t.into() }
}

struct Aligner<'s, F> {
    sa: &'s str,
    sb: &'s str,
    nested: F,
    da: HashSet<&'s str>,
    db: HashSet<&'s str>,
    diffs: Vec<TokenDiff>,
    /// (row in a, row in b) of every aligned leaf, in source order
    anchors: Vec<(usize, usize)>,
}

impl<'s, F: Fn(Node, &str) -> bool> Aligner<'s, F> {
    fn leaf(&mut self, a: Node, b: Node) {
        self.anchors.push((a.start_position().row, b.start_position().row));
        let (ta, tb) = (leaf_text(a, self.sa), leaf_text(b, self.sb));
        if ta == tb {
            return;
        }
        let kind = match a.kind() {
            "string_fragment" | "number" | "jsx_text" | "regex_pattern" | "escape_sequence" => DiffKind::Literal,
            "identifier" | "shorthand_property_identifier" | "shorthand_property_identifier_pattern" => {
                if self.da.contains(&*ta) && self.db.contains(&*tb) { DiffKind::Local } else { DiffKind::Name }
            }
            k if k.ends_with("identifier") => DiffKind::Name,
            _ => DiffKind::Code,
        };
        self.diffs.push(TokenDiff { a: a.byte_range(), b: b.byte_range(), kind });
    }

    /// Align two subtrees token for token; false = different structure.
    fn pair(&mut self, a: Node, b: Node) -> bool {
        if a.kind_id() != b.kind_id() {
            return false;
        }
        let nested = (self.nested)(a, self.sa);
        if nested != (self.nested)(b, self.sb) {
            return false;
        }
        if !nested {
            return self.pair_kids(a, b);
        }
        // a nested function is fingerprinted on its own, so it may differ: then mark it as a whole
        let mark = (self.diffs.len(), self.anchors.len());
        if !self.pair_kids(a, b) {
            self.diffs.truncate(mark.0);
            self.anchors.truncate(mark.1);
            self.diffs.push(TokenDiff { a: a.byte_range(), b: b.byte_range(), kind: DiffKind::Code });
            self.anchors.push((a.start_position().row, b.start_position().row));
            self.anchors.push((a.end_position().row, b.end_position().row));
        }
        true
    }

    fn pair_kids(&mut self, a: Node, b: Node) -> bool {
        let (ka, kb) = (kids(a), kids(b));
        if ka.is_empty() && kb.is_empty() {
            self.leaf(a, b);
            return true;
        }
        ka.len() == kb.len() && ka.into_iter().zip(kb).all(|(x, y)| self.pair(x, y))
    }
}

/// Rows that walk both line ranges in step, lining up each anchor pair on one row.
fn rows_from_anchors(anchors: &[(usize, usize)], ra: (usize, usize), rb: (usize, usize)) -> Vec<Row> {
    let mut rows = Vec::new();
    let (mut ea, mut eb) = (ra.0, rb.0);
    for &(la, lb) in anchors.iter().chain([&(ra.1, rb.1)]) {
        let (la, lb) = (la.clamp(ra.0, ra.1), lb.clamp(rb.0, rb.1));
        let da = if la >= ea { la - ea + 1 } else { 0 };
        let db = if lb >= eb { lb - eb + 1 } else { 0 };
        let m = da.max(db);
        // extra lines on the longer side go first, so the anchor lines share the last row
        for k in 0..m {
            let a = (k >= m - da).then(|| ea + k - (m - da));
            let b = (k >= m - db).then(|| eb + k - (m - db));
            rows.push(Row { a, b, changed: false });
        }
        ea += da;
        eb += db;
    }
    rows
}

/// Leaves of the fingerprinted subtrees, grouped by the line they start on (index = row - rows.0).
fn leaves_by_row<'t>(func: Node<'t>, fields: &[&str], rows: (usize, usize)) -> Vec<Vec<Node<'t>>> {
    let mut out = vec![Vec::new(); rows.1 + 1 - rows.0];
    let mut stack: Vec<Node> = roots_of(func, fields).into_iter().flatten().rev().collect();
    while let Some(n) = stack.pop() {
        let k = kids(n);
        if k.is_empty() {
            let r = n.start_position().row;
            if (rows.0..=rows.1).contains(&r) {
                out[r - rows.0].push(n);
            }
            continue;
        }
        stack.extend(k.into_iter().rev());
    }
    out
}

/// Line up two similar functions for a side-by-side view and list the tokens that differ.
/// `nested(n, src)` says whether `n` defines a separate symbol (as for [`shape`]).
pub fn align(a: Side, b: Side, fields: &[&str], nested: impl Fn(Node, &str) -> bool) -> Alignment {
    let mut al = Aligner {
        sa: a.src,
        sb: b.src,
        nested,
        da: declared(a.func, a.src),
        db: declared(b.func, b.src),
        diffs: Vec::new(),
        anchors: vec![(a.rows.0, b.rows.0)],
    };
    // same structure: pair the bodies token for token
    let exact = roots_of(a.func, fields).into_iter().zip(roots_of(b.func, fields)).all(|r| match r {
        (Some(x), Some(y)) => al.pair(x, y),
        (None, None) => true,
        _ => false,
    });
    if exact {
        let rows = rows_from_anchors(&al.anchors, a.rows, b.rows);
        return Alignment { exact, diffs: al.diffs, rows };
    }

    // otherwise match lines made of the same token kinds (longest common subsequence)
    al.diffs.clear();
    let la = leaves_by_row(a.func, fields, a.rows);
    let lb = leaves_by_row(b.func, fields, b.rows);
    let sig = |v: &Vec<Node>| v.iter().map(|n| n.kind_id()).collect::<Vec<u16>>();
    let (sa, sb): (Vec<_>, Vec<_>) = (la.iter().map(sig).collect(), lb.iter().map(sig).collect());
    let (n, m) = (sa.len(), sb.len());
    let mut matches: Vec<(usize, usize)> = Vec::new();
    if (n + 1) * (m + 1) <= MAX_LCS {
        let w = m + 1;
        let mut dp = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[i * w + j] =
                    if sa[i] == sb[j] { dp[(i + 1) * w + j + 1] + 1 } else { dp[(i + 1) * w + j].max(dp[i * w + j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if sa[i] == sb[j] {
                matches.push((i, j));
                i += 1;
                j += 1;
            } else if dp[(i + 1) * w + j] >= dp[i * w + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    let mut rows = Vec::new();
    let (mut i, mut j) = (0, 0);
    for &(mi, mj) in matches.iter().chain([&(n, m)]) {
        // unmatched lines between two matches sit side by side
        let gap = (mi - i).max(mj - j);
        for k in 0..gap {
            let ra = (i + k < mi).then(|| a.rows.0 + i + k);
            let rb = (j + k < mj).then(|| b.rows.0 + j + k);
            rows.push(Row { a: ra, b: rb, changed: true });
        }
        if mi < n {
            rows.push(Row { a: Some(a.rows.0 + mi), b: Some(b.rows.0 + mj), changed: false });
            for (x, y) in la[mi].iter().zip(&lb[mj]) {
                al.leaf(*x, *y);
            }
        }
        (i, j) = (mi + 1, mj + 1);
    }
    Alignment { exact: false, diffs: al.diffs, rows }
}
