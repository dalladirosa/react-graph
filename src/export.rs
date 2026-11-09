//! Text exports of the drawn graph: an SVG picture and a Mermaid flowchart.
//!
//! Both describe exactly what is on the canvas (the drawn cards and the links between them),
//! always in the light palette so the result reads the same wherever it is pasted.

use crate::graph::{Edge, EdgeKind, Graph, SymKind};
use std::collections::HashMap;
use std::fmt::Write;

/// What to export: the drawn cards, the links between them and where each card sits.
pub struct Drawn<'a> {
    pub graph: &'a Graph,
    pub nodes: &'a [usize],
    pub edges: &'a [Edge],
    pub pos: &'a HashMap<usize, (f32, f32)>,
    /// card size in canvas units
    pub card: (f32, f32),
}

const PAD: f32 = 24.0;
const INK: &str = "#1a192b";
const MUTED: &str = "#6b7280";
const BORDER: &str = "#e5e7eb";

fn kind_hex(k: SymKind) -> u32 {
    crate::kind_rgb(k)
}

// the same colours the canvas draws links with, in the light palette
fn edge_hex(k: EdgeKind) -> u32 {
    match k {
        EdgeKind::Renders => 0xb1b1b7,
        EdgeKind::Calls => 0xc4b5fd,
        EdgeKind::Handler => 0x86efac,
        EdgeKind::Defines => 0xd1d5db,
        EdgeKind::Context => 0xf59e0b,
    }
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// At most `max` characters, with an ellipsis when cut.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max.saturating_sub(1)).collect::<String>())
    }
}

fn kind_word(k: SymKind) -> &'static str {
    match k {
        SymKind::Component => "component",
        SymKind::Hook => "hook",
        SymKind::Function => "function",
    }
}

/// A standalone SVG: one card per symbol, one curve per link.
pub fn svg(d: &Drawn) -> String {
    let (cw, ch) = d.card;
    let (mut w, mut h) = (0f32, 0f32);
    for &i in d.nodes {
        if let Some(&(x, y)) = d.pos.get(&i) {
            w = w.max(x + cw);
            h = h.max(y + ch);
        }
    }
    let (w, h) = (w + 2.0 * PAD, h + 2.0 * PAD);
    let mut o = String::new();
    let _ = writeln!(
        o,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}" font-family="Segoe UI, system-ui, -apple-system, sans-serif">"#
    );
    let _ = writeln!(o, r##"<rect width="100%" height="100%" fill="#ffffff"/>"##);
    // one arrowhead per link colour, so a head always matches its curve
    let mut kinds: Vec<EdgeKind> = d.edges.iter().map(|e| e.kind).collect();
    kinds.sort_by_key(|k| edge_hex(*k));
    kinds.dedup();
    o.push_str("<defs>\n");
    for k in kinds {
        let _ = writeln!(
            o,
            r##"<marker id="a-{:06x}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" fill="#{:06x}"/></marker>"##,
            edge_hex(k),
            edge_hex(k)
        );
    }
    o.push_str("</defs>\n");

    // links first, so cards cover their ends
    for e in d.edges {
        let (Some(&(ax, ay)), Some(&(bx, by))) = (d.pos.get(&e.from), d.pos.get(&e.to)) else { continue };
        let (x1, y1) = (ax + cw + PAD, ay + ch / 2.0 + PAD);
        let (x2, y2) = (bx + PAD, by + ch / 2.0 + PAD);
        let bend = ((x2 - x1).abs() / 2.0).max(40.0);
        let dash = if matches!(e.kind, EdgeKind::Defines | EdgeKind::Context) { r#" stroke-dasharray="5 4""# } else { "" };
        let hex = edge_hex(e.kind);
        let _ = writeln!(
            o,
            r##"<path d="M{x1:.1} {y1:.1} C{:.1} {y1:.1} {:.1} {y2:.1} {x2:.1} {y2:.1}" fill="none" stroke="#{hex:06x}" stroke-width="1.6"{dash} marker-end="url(#a-{hex:06x})"><title>{}</title></path>"##,
            x1 + bend,
            x2 - bend,
            xml(&format!(
                "{} {} {}",
                d.graph.symbols[e.from].name,
                d.graph.edge_label(e),
                d.graph.symbols[e.to].name
            ))
        );
    }

    for &i in d.nodes {
        let Some(&(x, y)) = d.pos.get(&i) else { continue };
        let (x, y) = (x + PAD, y + PAD);
        let s = &d.graph.symbols[i];
        let color = kind_hex(s.kind);
        let _ = writeln!(
            o,
            r##"<g><rect x="{x:.1}" y="{y:.1}" width="{cw:.1}" height="{ch:.1}" rx="10" fill="#ffffff" stroke="{BORDER}"/><rect x="{x:.1}" y="{y:.1}" width="5" height="{ch:.1}" rx="2" fill="#{color:06x}"/><text x="{:.1}" y="{:.1}" font-size="14" font-weight="600" fill="{INK}">{}</text><text x="{:.1}" y="{:.1}" font-size="11" fill="{MUTED}">{}</text><text x="{:.1}" y="{:.1}" font-size="11" fill="{MUTED}">{}</text></g>"##,
            x + 16.0,
            y + 25.0,
            xml(&clip(&s.name, 24)),
            x + 16.0,
            y + 43.0,
            xml(&format!("{} · line {}", kind_word(s.kind), s.line)),
            x + 16.0,
            y + 57.0,
            xml(&clip(&s.file, 32)),
        );
    }
    o.push_str("</svg>\n");
    o
}

/// A Mermaid `flowchart LR`: components are boxes, hooks stadiums, functions rounded boxes.
pub fn mermaid(d: &Drawn) -> String {
    let mut o = String::from("flowchart LR\n");
    let label = |s: &str| s.replace('"', "'");
    for &i in d.nodes {
        let s = &d.graph.symbols[i];
        let name = label(&s.name);
        let _ = match s.kind {
            SymKind::Component => writeln!(o, "  n{i}[\"{name}\"]"),
            SymKind::Hook => writeln!(o, "  n{i}([\"{name}\"])"),
            SymKind::Function => writeln!(o, "  n{i}(\"{name}\")"),
        };
    }
    for e in d.edges {
        let text = label(&d.graph.edge_label(e));
        let _ = match e.kind {
            EdgeKind::Renders => writeln!(o, "  n{} -->|{text}| n{}", e.from, e.to),
            EdgeKind::Calls => writeln!(o, "  n{} -->|{text}| n{}", e.from, e.to),
            EdgeKind::Handler => writeln!(o, "  n{} ==>|{text}| n{}", e.from, e.to),
            EdgeKind::Defines => writeln!(o, "  n{} -.-|{text}| n{}", e.from, e.to),
            EdgeKind::Context => writeln!(o, "  n{} -.->|\"{text}\"| n{}", e.from, e.to),
        };
    }
    for (class, kind) in [("component", SymKind::Component), ("hook", SymKind::Hook), ("function", SymKind::Function)] {
        let ids: Vec<String> =
            d.nodes.iter().filter(|&&i| d.graph.symbols[i].kind == kind).map(|i| format!("n{i}")).collect();
        if !ids.is_empty() {
            let _ = writeln!(o, "  classDef {class} stroke:#{:06x},stroke-width:2px", kind_hex(kind));
            let _ = writeln!(o, "  class {} {class}", ids.join(","));
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph;
    use std::path::Path;

    fn drawn_all(g: &Graph) -> (Vec<usize>, Vec<Edge>, HashMap<usize, (f32, f32)>) {
        let nodes: Vec<usize> = (0..g.symbols.len()).collect();
        let pos = nodes.iter().map(|&i| (i, (i as f32 * 230.0, (i % 3) as f32 * 90.0))).collect();
        (nodes, g.edges.clone(), pos)
    }

    #[test]
    fn context_fixture_exports() {
        let g = graph::build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/context"));
        let (nodes, edges, pos) = drawn_all(&g);
        let d = Drawn { graph: &g, nodes: &nodes, edges: &edges, pos: &pos, card: (200.0, 66.0) };

        let m = mermaid(&d);
        assert!(m.starts_with("flowchart LR\n"));
        let id = |name: &str| g.symbols.iter().position(|s| s.name == name).unwrap();
        assert!(m.contains(&format!("n{}[\"App\"]", id("App"))), "{m}");
        assert!(m.contains(&format!("n{}([\"useTheme\"])", id("useTheme"))), "{m}");
        assert!(m.contains(&format!("n{} -->|renders| n{}", id("App"), id("Header"))), "{m}");
        assert!(m.contains(&format!("n{} -.->|\"context · ThemeContext\"| n{}", id("App"), id("Footer"))), "{m}");
        assert!(m.contains("class n"), "{m}");

        let s = svg(&d);
        assert!(s.starts_with("<svg ") && s.trim_end().ends_with("</svg>"));
        assert_eq!(s.matches("<g>").count(), nodes.len());
        assert_eq!(s.matches("marker-end").count(), edges.len());
        assert!(s.contains(">App</text>") && s.contains("component · line "));
        // every opened tag is closed (cheap well-formedness check)
        assert_eq!(s.matches("<path").count(), s.matches("</path>").count() + s.matches("<path d=\"M0 0").count());
    }

    #[test]
    fn text_is_escaped_and_clipped() {
        assert_eq!(xml("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("abcdefghij", 5), "abcd…");
    }
}
