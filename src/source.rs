//! Loads a source file and syntax-highlights it (light theme) for the code panel.

use crate::rgb;
use gpui::{FontStyle, FontWeight, HighlightStyle, Hsla, SharedString};
use std::ops::Range;
use std::path::Path;
use tree_sitter::{Node, Parser};

pub struct SourceFile {
    /// one entry per line, tabs expanded, no line terminators
    pub lines: Vec<SharedString>,
    /// highlight ranges per line (byte offsets into that line), sorted, non-overlapping
    pub highlights: Vec<Vec<(Range<usize>, HighlightStyle)>>,
    /// index of the longest line (used to size the horizontal scroll)
    pub widest: usize,
}

#[derive(Clone, Copy)]
enum Tok {
    Comment,
    Keyword,
    String,
    Number,
    Constant,
    Type,
    Function,
    Tag,
    Attr,
}

fn style(t: Tok) -> HighlightStyle {
    let c = |hex: u32| -> Option<Hsla> { Some(rgb(hex).into()) };
    // GitHub light palette
    match t {
        Tok::Comment => HighlightStyle { color: c(0x6e7781), font_style: Some(FontStyle::Italic), ..Default::default() },
        Tok::Keyword => HighlightStyle { color: c(0xcf222e), ..Default::default() },
        Tok::String => HighlightStyle { color: c(0x0a3069), ..Default::default() },
        Tok::Number | Tok::Constant => HighlightStyle { color: c(0x0550ae), ..Default::default() },
        Tok::Type => HighlightStyle { color: c(0x953800), ..Default::default() },
        Tok::Function => HighlightStyle { color: c(0x8250df), ..Default::default() },
        Tok::Tag => HighlightStyle { color: c(0x116329), font_weight: Some(FontWeight::MEDIUM), ..Default::default() },
        Tok::Attr => HighlightStyle { color: c(0x0550ae), ..Default::default() },
    }
}

fn classify(n: Node) -> Option<Tok> {
    let kind = n.kind();
    let parent = n.parent().map(|p| p.kind()).unwrap_or("");
    let field_of = |field: &str| {
        n.parent()
            .and_then(|p| p.child_by_field_name(field))
            .is_some_and(|c| c.id() == n.id())
    };
    match kind {
        "comment" => Some(Tok::Comment),
        "string" | "template_string" | "regex" => Some(Tok::String),
        "number" => Some(Tok::Number),
        "true" | "false" | "null" | "undefined" | "this" | "super" => Some(Tok::Constant),
        "type_identifier" | "predefined_type" => Some(Tok::Type),
        "identifier" | "member_expression" | "nested_identifier"
            if matches!(parent, "jsx_opening_element" | "jsx_closing_element" | "jsx_self_closing_element") =>
        {
            Some(Tok::Tag)
        }
        "property_identifier" if parent == "jsx_attribute" => Some(Tok::Attr),
        "identifier" if parent == "call_expression" && field_of("function") => Some(Tok::Function),
        "property_identifier" if parent == "member_expression" && field_of("property") => {
            let is_call = n
                .parent()
                .and_then(|m| m.parent().map(|c| (m, c)))
                .is_some_and(|(m, c)| {
                    c.kind() == "call_expression" && c.child_by_field_name("function").is_some_and(|f| f.id() == m.id())
                });
            is_call.then_some(Tok::Function)
        }
        "identifier" if parent == "function_declaration" && field_of("name") => Some(Tok::Function),
        "identifier" if parent == "variable_declarator" && field_of("name") => {
            let v = n.parent().and_then(|p| p.child_by_field_name("value"));
            v.is_some_and(|v| matches!(v.kind(), "arrow_function" | "function_expression" | "function"))
                .then_some(Tok::Function)
        }
        _ if !n.is_named() && kind.chars().all(|c| c.is_ascii_alphabetic()) && !kind.is_empty() => Some(Tok::Keyword),
        "=>" => Some(Tok::Keyword),
        _ => None,
    }
}

fn walk(n: Node, out: &mut Vec<(Range<usize>, Tok)>) {
    if let Some(t) = classify(n) {
        if n.end_byte() > n.start_byte() {
            out.push((n.start_byte()..n.end_byte(), t));
        }
        return;
    }
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        walk(ch, out);
    }
}

/// The text as the code panel shows it: `\n` line ends, tabs expanded to 4 spaces.
pub fn normalize(raw: &str) -> String {
    raw.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ")
}

/// Maps a byte offset in the raw file to the same place in [`normalize`]d text.
pub struct OffsetMap {
    /// (raw offset of a `\t` or of the `\r` in `\r\n`, size change it causes)
    shifts: Vec<(usize, isize)>,
}

impl OffsetMap {
    pub fn new(raw: &str) -> Self {
        let b = raw.as_bytes();
        let shifts = (0..b.len())
            .filter_map(|i| match b[i] {
                b'\t' => Some((i, 3)),
                b'\r' if b.get(i + 1) == Some(&b'\n') => Some((i, -1)),
                _ => None,
            })
            .collect();
        OffsetMap { shifts }
    }

    pub fn map(&self, raw: usize) -> usize {
        let n = self.shifts.partition_point(|&(p, _)| p < raw);
        (raw as isize + self.shifts[..n].iter().map(|s| s.1).sum::<isize>()) as usize
    }
}

pub fn load(path: &Path) -> Result<SourceFile, String> {
    let raw = std::fs::read(path).map_err(|e| format!("Can't read {}: {e}", path.display()))?;
    Ok(from_text(&normalize(&String::from_utf8_lossy(&raw))))
}

/// Split already [`normalize`]d text into highlighted lines.
pub fn from_text(text: &str) -> SourceFile {
    let mut spans: Vec<(Range<usize>, Tok)> = Vec::new();
    let mut parser = Parser::new();
    if parser.set_language(&tree_sitter_typescript::LANGUAGE_TSX.into()).is_ok() {
        if let Some(tree) = parser.parse(text, None) {
            walk(tree.root_node(), &mut spans);
        }
    }

    let mut lines = Vec::new();
    let mut highlights = Vec::new();
    let mut widest = (0, 0);
    let mut si = 0;
    let mut start = 0;
    for (ln, line) in text.split('\n').enumerate() {
        let end = start + line.len();
        // skip spans that ended before this line
        while si < spans.len() && spans[si].0.end <= start {
            si += 1;
        }
        let mut hl = Vec::new();
        let mut k = si;
        while k < spans.len() && spans[k].0.start < end {
            let (r, t) = &spans[k];
            let (a, b) = (r.start.max(start), r.end.min(end));
            if b > a {
                hl.push((a - start..b - start, style(*t)));
            }
            k += 1;
        }
        let w = line.chars().count();
        if w > widest.1 {
            widest = (ln, w);
        }
        lines.push(SharedString::from(line.to_string()));
        highlights.push(hl);
        start = end + 1;
    }
    // drop the empty line produced by a trailing newline
    if lines.len() > 1 && lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
        highlights.pop();
    }
    SourceFile { lines, highlights, widest: widest.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_sample_component() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../sample-react/src/components/CartPanel.tsx");
        let f = load(&p).expect("load");
        assert_eq!(f.lines.len(), f.highlights.len());
        for (line, hl) in f.lines.iter().zip(&f.highlights) {
            let mut last = 0;
            for (r, _) in hl {
                assert!(r.start >= last && r.end <= line.len() && r.start < r.end, "bad range {r:?} in {line:?}");
                assert!(line.is_char_boundary(r.start) && line.is_char_boundary(r.end));
                last = r.end;
            }
        }
        for (i, (line, hl)) in f.lines.iter().zip(&f.highlights).enumerate().take(14) {
            let toks: Vec<&str> = hl.iter().map(|(r, _)| &line[r.clone()]).collect();
            println!("{:>3} {line}\n      {toks:?}", i + 1);
        }
    }
}
