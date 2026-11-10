//! Text exports of the drawn graph: an SVG picture, a Mermaid flowchart and a self-contained HTML viewer.
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

/// Size of the drawing, padding included.
fn extent(d: &Drawn) -> (f32, f32) {
    let (cw, ch) = d.card;
    let (mut w, mut h) = (0f32, 0f32);
    for &i in d.nodes {
        if let Some(&(x, y)) = d.pos.get(&i) {
            w = w.max(x + cw);
            h = h.max(y + ch);
        }
    }
    (w + 2.0 * PAD, h + 2.0 * PAD)
}

/// A standalone SVG: one card per symbol, one curve per link.
pub fn svg(d: &Drawn) -> String {
    svg_with(d, false)
}

/// `ids`: for the HTML viewer. The picture fills its container, everything sits in a `<g id="vp">`
/// the viewer moves and scales, and cards and links carry the symbol indexes they stand for.
fn svg_with(d: &Drawn, ids: bool) -> String {
    let (cw, ch) = d.card;
    let (w, h) = extent(d);
    let mut o = String::new();
    if ids {
        let _ = writeln!(
            o,
            r#"<svg xmlns="http://www.w3.org/2000/svg" class="graph" width="100%" height="100%" font-family="Segoe UI, system-ui, -apple-system, sans-serif">"#
        );
    } else {
        let _ = writeln!(
            o,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}" font-family="Segoe UI, system-ui, -apple-system, sans-serif">"#
        );
        let _ = writeln!(o, r##"<rect width="100%" height="100%" fill="#ffffff"/>"##);
    }
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
    if ids {
        o.push_str("<g id=\"vp\">\n");
    }

    // links first, so cards cover their ends
    for e in d.edges {
        let (Some(&(ax, ay)), Some(&(bx, by))) = (d.pos.get(&e.from), d.pos.get(&e.to)) else { continue };
        let (x1, y1) = (ax + cw + PAD, ay + ch / 2.0 + PAD);
        let (x2, y2) = (bx + PAD, by + ch / 2.0 + PAD);
        let bend = ((x2 - x1).abs() / 2.0).max(40.0);
        let dash = if matches!(e.kind, EdgeKind::Defines | EdgeKind::Context) { r#" stroke-dasharray="5 4""# } else { "" };
        let hex = edge_hex(e.kind);
        let ea = if ids { format!(r#" class="edge" data-f="{}" data-t="{}""#, e.from, e.to) } else { String::new() };
        let _ = writeln!(
            o,
            r##"<path{ea} d="M{x1:.1} {y1:.1} C{:.1} {y1:.1} {:.1} {y2:.1} {x2:.1} {y2:.1}" fill="none" stroke="#{hex:06x}" stroke-width="1.6"{dash} marker-end="url(#a-{hex:06x})"><title>{}</title></path>"##,
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
        let ga = if ids { format!(r#" class="card" data-i="{i}""#) } else { String::new() };
        let _ = writeln!(
            o,
            r##"<g{ga}><rect x="{x:.1}" y="{y:.1}" width="{cw:.1}" height="{ch:.1}" rx="10" fill="#ffffff" stroke="{BORDER}"/><rect x="{x:.1}" y="{y:.1}" width="5" height="{ch:.1}" rx="2" fill="#{color:06x}"/><text x="{:.1}" y="{:.1}" font-size="14" font-weight="600" fill="{INK}">{}</text><text x="{:.1}" y="{:.1}" font-size="11" fill="{MUTED}">{}</text><text x="{:.1}" y="{:.1}" font-size="11" fill="{MUTED}">{}</text></g>"##,
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
    if ids {
        o.push_str("</g>\n");
    }
    o.push_str("</svg>\n");
    o
}

/// JSON that is safe inside a `<script>` element: no `</script>` or `<!--` can end it early.
fn script_json(v: &serde_json::Value) -> String {
    v.to_string().replace('<', "\\u003c").replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029")
}

/// One self-contained HTML page: the drawn graph as an SVG you can pan and zoom, search, and click
/// through (selecting a card lights up everything it uses and everything that uses it). No network,
/// no dependencies: send it to someone, or attach it to a pull request.
pub fn html(d: &Drawn, title: &str) -> String {
    let (w, h) = extent(d);
    let g = d.graph;
    let nodes: Vec<serde_json::Value> = d
        .nodes
        .iter()
        .map(|&i| {
            let s = &g.symbols[i];
            serde_json::json!({
                "i": i,
                "name": s.name,
                "kind": kind_word(s.kind),
                "file": s.file,
                "line": s.line,
                "wraps": s.wraps.iter().map(|w| w.label()).collect::<Vec<_>>(),
                "warnings": s.warnings(),
                "similar": s.similar.len(),
                "cycle": s.cycle.is_some(),
            })
        })
        .collect();
    let edges: Vec<serde_json::Value> = d
        .edges
        .iter()
        .map(|e| serde_json::json!({ "f": e.from, "t": e.to, "k": g.edge_label(e) }))
        .collect();
    let data = serde_json::json!({ "title": title, "w": w, "h": h, "nodes": nodes, "edges": edges });
    HTML_PAGE
        .replace("{{TITLE}}", &xml(title))
        .replace("{{SVG}}", &svg_with(d, true))
        .replace("{{DATA}}", &script_json(&data))
}

const HTML_PAGE: &str = r####"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{TITLE}} · React Graph</title>
<style>
  :root { --ink:#1a192b; --muted:#6b7280; --line:#e5e7eb; --bg:#f8f9fb; --accent:#ff0071; }
  * { box-sizing: border-box; }
  html, body { height: 100%; margin: 0; }
  body { display: flex; flex-direction: column; font: 14px/1.4 "Segoe UI", system-ui, -apple-system, sans-serif; color: var(--ink); background: var(--bg); }
  header { display: flex; flex-wrap: wrap; align-items: center; gap: 12px; padding: 8px 14px; background: #fff; border-bottom: 1px solid var(--line); }
  header h1 { font-size: 15px; margin: 0; }
  header .count { color: var(--muted); font-size: 12px; }
  header input { flex: 0 1 260px; padding: 5px 9px; border: 1px solid #d3d5db; border-radius: 6px; font: inherit; }
  header input:focus { outline: 2px solid var(--accent); outline-offset: -1px; }
  header button { padding: 4px 10px; border: 1px solid #d3d5db; border-radius: 6px; background: #fff; font: inherit; cursor: pointer; }
  header button:hover { background: #f3f4f6; }
  .legend { display: flex; gap: 10px; margin-left: auto; color: var(--muted); font-size: 12px; }
  .legend i { display: inline-block; width: 8px; height: 8px; border-radius: 50%; margin-right: 4px; }
  main { flex: 1; display: flex; min-height: 0; }
  #stage { flex: 1; min-width: 0; background: #fffffd; background-image: radial-gradient(#e5e7eb 1px, transparent 1px); background-size: 20px 20px; touch-action: none; cursor: grab; }
  #stage.drag { cursor: grabbing; }
  aside { width: 300px; flex-shrink: 0; overflow: auto; padding: 14px; background: #fff; border-left: 1px solid var(--line); }
  aside h2 { margin: 0 0 2px; font-size: 16px; word-break: break-word; }
  aside .meta { color: var(--muted); font-size: 12px; word-break: break-all; }
  aside h3 { margin: 14px 0 4px; font-size: 11px; letter-spacing: .04em; text-transform: uppercase; color: var(--muted); }
  aside ul { list-style: none; margin: 0; padding: 0; }
  aside li { padding: 3px 6px; border-radius: 5px; cursor: pointer; display: flex; gap: 6px; justify-content: space-between; }
  aside li:hover { background: #fdf2f8; color: var(--accent); }
  aside li small { color: var(--muted); }
  .tag { display: inline-block; padding: 0 5px; margin: 0 4px 2px 0; border-radius: 4px; background: #f1f5f9; font: 11px ui-monospace, Consolas, monospace; }
  .tag.warn { background: #fef3c7; color: #92400e; }
  .tag.dup { background: #ede9fe; color: #6d28d9; }
  .tag.cyc { background: #fce7f3; color: #be185d; }
  .hint { color: var(--muted); font-size: 12px; }
  .card { cursor: pointer; }
  .card.dim, .edge.dim { opacity: .15; }
  .card.sel rect:first-child { stroke: var(--accent); stroke-width: 2; }
  .card.hit rect:first-child { stroke: var(--accent); }
  .edge.hot { stroke-width: 2.6; }
  @media (max-width: 720px) { aside { display: none; } .legend { display: none; } }
</style>
</head>
<body>
<header>
  <h1>{{TITLE}}</h1>
  <span class="count" id="count"></span>
  <input id="q" type="search" placeholder="Search symbols  ( / )" autocomplete="off">
  <button id="fit" type="button">Fit</button>
  <span class="legend"><span><i style="background:#2563eb"></i>component</span><span><i style="background:#7c3aed"></i>hook</span><span><i style="background:#0d9488"></i>function</span></span>
</header>
<main>
  <div id="stage">
{{SVG}}  </div>
  <aside id="panel"><p class="hint">Click a card to see what it uses and what uses it. Drag to pan, scroll to zoom, double-click to fit. Esc clears.</p></aside>
</main>
<script id="data" type="application/json">{{DATA}}</script>
<script>
(function () {
  "use strict";
  var D = JSON.parse(document.getElementById("data").textContent);
  var stage = document.getElementById("stage"), svg = stage.querySelector("svg"), vp = document.getElementById("vp");
  var panel = document.getElementById("panel"), q = document.getElementById("q");
  var byId = {}, out = {}, inn = {}, cards = {}, edgeEls = [];
  D.nodes.forEach(function (n) { byId[n.i] = n; out[n.i] = []; inn[n.i] = []; });
  D.edges.forEach(function (e) { if (out[e.f] && inn[e.t]) { out[e.f].push(e); inn[e.t].push(e); } });
  svg.querySelectorAll(".card").forEach(function (c) { cards[c.getAttribute("data-i")] = c; });
  svg.querySelectorAll(".edge").forEach(function (p, k) { edgeEls.push(p); });
  document.getElementById("count").textContent = D.nodes.length + " symbols · " + D.edges.length + " links";

  var k = 1, tx = 0, ty = 0, sel = null;
  function apply() { vp.setAttribute("transform", "translate(" + tx + " " + ty + ") scale(" + k + ")"); }
  function fit() {
    var r = stage.getBoundingClientRect();
    if (!D.w || !D.h || !r.width) { k = 1; tx = 0; ty = 0; return apply(); }
    k = Math.min(r.width / D.w, r.height / D.h, 1);
    tx = (r.width - D.w * k) / 2; ty = (r.height - D.h * k) / 2; apply();
  }
  function center(i) {
    var c = cards[i]; if (!c) return;
    var r = stage.getBoundingClientRect(), b = c.getBBox();
    k = Math.max(k, 0.8); touched = true;
    tx = r.width / 2 - (b.x + b.width / 2) * k; ty = r.height / 2 - (b.y + b.height / 2) * k; apply();
  }
  function reach(i, next) {
    var seen = {}; seen[i] = true; var stack = [i];
    while (stack.length) { var n = stack.pop(); (next[n] || []).forEach(function (e) { var m = next === out ? e.t : e.f; if (!seen[m]) { seen[m] = true; stack.push(m); } }); }
    return seen;
  }
  function clearMarks() {
    Object.keys(cards).forEach(function (i) { cards[i].classList.remove("dim", "sel", "hit"); });
    edgeEls.forEach(function (p) { p.classList.remove("dim", "hot"); });
  }
  function li(text, small, id) {
    var el = document.createElement("li"); el.appendChild(document.createTextNode(text));
    var s = document.createElement("small"); s.textContent = small; el.appendChild(s);
    el.addEventListener("click", function () { select(id, true); }); return el;
  }
  function section(title, list) {
    var h = document.createElement("h3"); h.textContent = title; panel.appendChild(h);
    var ul = document.createElement("ul"); list.forEach(function (x) { ul.appendChild(x); }); panel.appendChild(ul);
    if (!list.length) { var p = document.createElement("p"); p.className = "hint"; p.textContent = "Nothing"; panel.insertBefore(p, ul); }
  }
  function select(i, focus) {
    clearMarks(); sel = i; panel.textContent = "";
    if (i === null || !byId[i]) { sel = null; var p = document.createElement("p"); p.className = "hint"; p.textContent = "Click a card to see what it uses and what uses it."; panel.appendChild(p); return; }
    var n = byId[i], up = reach(i, inn), down = reach(i, out);
    Object.keys(cards).forEach(function (j) { if (!up[j] && !down[j]) cards[j].classList.add("dim"); });
    edgeEls.forEach(function (p) {
      var f = p.getAttribute("data-f"), t = p.getAttribute("data-t");
      var on = (up[f] && up[t]) || (down[f] && down[t]);
      if (!on) p.classList.add("dim"); else if (f == i || t == i) p.classList.add("hot");
    });
    cards[i].classList.add("sel");
    var h = document.createElement("h2"); h.textContent = n.name; panel.appendChild(h);
    var m = document.createElement("div"); m.className = "meta"; m.textContent = n.kind + " · " + n.file + ":" + n.line; panel.appendChild(m);
    var tags = document.createElement("div"); tags.style.marginTop = "6px";
    function tag(t, c) { var s = document.createElement("span"); s.className = "tag" + (c ? " " + c : ""); s.textContent = t; tags.appendChild(s); }
    n.wraps.forEach(function (w) { tag(w); });
    if (n.warnings) tag(n.warnings + " warning" + (n.warnings > 1 ? "s" : ""), "warn");
    if (n.similar) tag("similar code", "dup");
    if (n.cycle) tag("in a cycle", "cyc");
    panel.appendChild(tags);
    section("Uses", out[i].map(function (e) { return li(byId[e.t].name, e.k, e.t); }));
    section("Used by", inn[i].map(function (e) { return li(byId[e.f].name, e.k, e.f); }));
    if (focus) center(i);
  }
  function search() {
    var v = q.value.trim().toLowerCase(); clearMarks(); sel = null;
    if (!v) { select(null); return []; }
    var hits = D.nodes.filter(function (n) { return n.name.toLowerCase().indexOf(v) >= 0 || n.file.toLowerCase().indexOf(v) >= 0; });
    var set = {}; hits.forEach(function (n) { set[n.i] = true; });
    Object.keys(cards).forEach(function (i) { cards[i].classList.add(set[i] ? "hit" : "dim"); });
    edgeEls.forEach(function (p) { p.classList.add("dim"); });
    panel.textContent = ""; var p = document.createElement("p"); p.className = "hint"; p.textContent = hits.length + " match" + (hits.length === 1 ? "" : "es") + ". Enter jumps to the first."; panel.appendChild(p);
    return hits;
  }
  q.addEventListener("input", search);
  q.addEventListener("keydown", function (e) { if (e.key === "Enter") { var h = search(); if (h.length) { select(h[0].i, true); } } if (e.key === "Escape") { q.value = ""; select(null); q.blur(); } });
  document.addEventListener("keydown", function (e) {
    if (e.key === "/" && document.activeElement !== q) { e.preventDefault(); q.focus(); }
    else if (e.key === "Escape" && document.activeElement !== q) { select(null); }
  });
  function fitAgain() { touched = false; fit(); }
  document.getElementById("fit").addEventListener("click", fitAgain);
  svg.addEventListener("dblclick", fitAgain);
  svg.addEventListener("click", function (e) {
    var c = e.target.closest ? e.target.closest(".card") : null;
    if (c) select(Number(c.getAttribute("data-i"))); else if (!moved) select(null);
  });
  var drag = null, moved = false, touched = false;
  stage.addEventListener("pointerdown", function (e) { if (e.button !== 0) return; drag = { x: e.clientX, y: e.clientY, tx: tx, ty: ty }; moved = false; stage.setPointerCapture(e.pointerId); });
  stage.addEventListener("pointermove", function (e) {
    if (!drag) return; var dx = e.clientX - drag.x, dy = e.clientY - drag.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) { moved = true; touched = true; stage.classList.add("drag"); }
    tx = drag.tx + dx; ty = drag.ty + dy; apply();
  });
  function end() { drag = null; stage.classList.remove("drag"); setTimeout(function () { moved = false; }, 0); }
  stage.addEventListener("pointerup", end); stage.addEventListener("pointercancel", end);
  stage.addEventListener("wheel", function (e) {
    e.preventDefault(); var r = stage.getBoundingClientRect(), mx = e.clientX - r.left, my = e.clientY - r.top;
    var k2 = Math.min(4, Math.max(0.05, k * Math.exp(-e.deltaY * 0.0015)));
    tx = mx - (mx - tx) * (k2 / k); ty = my - (my - ty) * (k2 / k); k = k2; touched = true; apply();
  }, { passive: false });
  // keep the whole graph in view until the user moves it (the container may get its size after the script runs)
  function refit() { if (!touched) fit(); }
  window.addEventListener("resize", refit);
  window.addEventListener("load", refit);
  if (window.ResizeObserver) new ResizeObserver(refit).observe(stage);
  fit();
})();
</script>
</body>
</html>
"####;

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
    fn html_viewer_is_self_contained() {
        let g = graph::build(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/context"));
        let (nodes, edges, pos) = drawn_all(&g);
        let d = Drawn { graph: &g, nodes: &nodes, edges: &edges, pos: &pos, card: (200.0, 66.0) };
        let h = html(&d, "my <app> & co");
        assert!(h.starts_with("<!doctype html>") && h.trim_end().ends_with("</html>"));
        // the title is escaped, and nothing is loaded from elsewhere
        assert!(h.contains("<title>my &lt;app&gt; &amp; co · React Graph</title>"));
        assert!(!h.contains("http://") || h.matches("http://").count() == 1, "only the svg namespace");
        assert!(!h.contains("https://") && !h.contains("src=") && !h.contains("@import"));
        // one card and one link per drawn symbol and edge, with the indexes the data refers to
        assert_eq!(h.matches("class=\"card\" data-i=").count(), nodes.len());
        assert_eq!(h.matches("class=\"edge\" data-f=").count(), edges.len());
        assert_eq!(h.matches("<g id=\"vp\">").count(), 1);
        // the embedded data parses and matches
        let start = h.find("<script id=\"data\" type=\"application/json\">").unwrap() + "<script id=\"data\" type=\"application/json\">".len();
        let end = start + h[start..].find("</script>").unwrap();
        let v: serde_json::Value = serde_json::from_str(&h[start..end]).unwrap();
        assert_eq!(v["nodes"].as_array().unwrap().len(), nodes.len());
        assert_eq!(v["edges"].as_array().unwrap().len(), edges.len());
        assert_eq!(v["title"], "my <app> & co");
        // the plain SVG export is unchanged by the viewer's extra attributes
        assert!(!svg(&d).contains("data-i="));
    }

    #[test]
    fn html_of_an_empty_graph_still_works() {
        let g = Graph::default();
        let pos = HashMap::new();
        let d = Drawn { graph: &g, nodes: &[], edges: &[], pos: &pos, card: (200.0, 66.0) };
        let h = html(&d, "empty");
        assert!(h.contains("\"nodes\":[]") && h.contains("<g id=\"vp\">"));
    }

    #[test]
    fn script_json_cannot_close_its_script() {
        let v = serde_json::json!({ "file": "</script><script>alert(1)</script>", "a": "x\u{2028}y" });
        let s = script_json(&v);
        assert!(!s.contains("</") && !s.contains('<') && !s.contains('\u{2028}'), "{s}");
        // and it still means the same thing
        let back: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn text_is_escaped_and_clipped() {
        assert_eq!(xml("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("abcdefghij", 5), "abcd…");
    }
}
