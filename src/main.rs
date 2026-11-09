// release builds are a GUI app: no console window behind it
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod alias;
mod compare;
mod drill;
mod export;
mod flow;
mod graph;
mod prefs;
mod similar;
mod source;

use gpui::{
    App, Application, Bounds, ClickEvent, Context, Div, FocusHandle, Hsla, IntoElement, KeyDownEvent,
    ListHorizontalSizingBehavior, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, PathPromptOptions, Pixels, Point,
    Render, ScrollStrategy, ScrollWheelEvent, Stateful, StyledText, UniformListScrollHandle, Window, WindowBounds,
    WindowOptions, anchored, canvas, deferred, div, fill, point, prelude::*, px, size, uniform_list,
};
use compare::Compare;
use flow::{Frame, Move, Start, Style, Trace};
use graph::{DepInfo, Edge, EdgeKind, Graph, Origin, SymKind, Wrap};
use similar::DiffKind;
use source::SourceFile;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

const NODE_W: f32 = 200.0;
const NODE_H: f32 = 66.0;
const COL_GAP: f32 = 120.0;
const ROW_GAP: f32 = 26.0;
/// below this zoom cards have no text, and are painted on one canvas instead of built as elements
const TEXT_ZOOM: f32 = 0.35;

// overlay geometry (screen px)
const MARGIN: f32 = 16.0;
const LEFT_W: f32 = 300.0;
const DETAIL_W: f32 = 340.0;
const MM_W: f32 = 200.0;
const MM_H: f32 = 140.0;
/// space kept free for the toolbar above the canvas when fitting
const TOP_BAR: f32 = 96.0;

// React Flow pink (sampled from reactflow.dev), with Tailwind-style lighter / darker shades
const ACCENT: u32 = 0xff0071;
const ACCENT_DARK: u32 = 0xe00063;
const ACCENT_50: u32 = 0xfdf2f8;
const ACCENT_100: u32 = 0xfce7f3;
const ACCENT_200: u32 = 0xfbcfe8;
const ACCENT_300: u32 = 0xf9a8d4;
const ACCENT_400: u32 = 0xf472b6;
/// canvas background: white in light mode, the darkest surface in dark mode
const CANVAS: u32 = 0xfffffd;
/// text / icons on an accent fill: white in both modes
const ON_ACCENT: u32 = 0xfffffe;
/// neutral hover fill (menus, list rows)
const HOVER: u32 = 0xf3f4f6;
/// title bar (the window draws its own caption, JetBrains-style)
const TITLE_BG: u32 = 0xf7f8fa;
const TITLE_HOVER: u32 = 0xebecf0;
const TB: f32 = 40.0;
const ICON_FONT: &str = "Segoe Fluent Icons";
const MAC: bool = cfg!(target_os = "macos");
const UI_FONT: &str = if cfg!(windows) { "Segoe UI" } else { ".SystemUIFont" };
const MONO_FONT: &str = if cfg!(windows) { "Consolas" } else if MAC { "Menlo" } else { "DejaVu Sans Mono" };

/// Segoe Fluent Icons only ships with Windows; elsewhere the same icons come from bundled SVGs.
/// `REACT_GRAPH_SVG_ICONS=1` uses the SVGs on Windows too (to check them without a Mac).
fn svg_icons() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !cfg!(windows) || std::env::var_os("REACT_GRAPH_SVG_ICONS").is_some())
}

/// Bundled SVG for a Segoe Fluent Icons glyph.
fn icon_svg(glyph: &str) -> &'static str {
    match glyph {
        "\u{E70D}" => "icons/chevron-down.svg",
        "\u{E76C}" => "icons/chevron-right.svg",
        "\u{E721}" => "icons/search.svg",
        "\u{E73F}" => "icons/minimize-2.svg",
        "\u{E7BA}" => "icons/triangle-alert.svg",
        "\u{E81E}" => "icons/layers.svg",
        "\u{E894}" => "icons/filter-x.svg",
        "\u{E8A5}" => "icons/file.svg",
        "\u{E8B7}" => "icons/folder.svg",
        "\u{E8C8}" => "icons/copy.svg",
        "\u{E945}" => "icons/zap.svg",
        "\u{E8EE}" => "icons/repeat.svg",
        "\u{ED1A}" => "icons/eye-off.svg",
        "\u{E706}" => "icons/sun.svg",
        "\u{E708}" => "icons/moon.svg",
        _ => "icons/x.svg",
    }
}

/// An icon at `size` px. `color: None` takes the parent's text colour (hover included) where the
/// platform allows it; SVG icons can't inherit, so they fall back to MUTED.
fn icon(glyph: &'static str, size: f32, color: Option<u32>) -> gpui::AnyElement {
    if svg_icons() {
        gpui::svg()
            .path(icon_svg(glyph))
            .flex_shrink_0()
            .size(px((size * 1.15).round()))
            .text_color(rgb(color.unwrap_or(MUTED)))
            .into_any_element()
    } else {
        div()
            .flex_shrink_0()
            .font_family(ICON_FONT)
            .text_size(px(size))
            .when_some(color, |d, c| d.text_color(rgb(c)))
            .child(glyph)
            .into_any_element()
    }
}

macro_rules! bundled {
    ($($name:literal),* $(,)?) => {
        &[$((concat!("icons/", $name), include_bytes!(concat!("../assets/icons/", $name)) as &[u8])),*]
    };
}
const ICONS: &[(&str, &[u8])] = bundled!(
    "chevron-down.svg", "chevron-right.svg", "copy.svg", "eye-off.svg", "file.svg", "filter-x.svg", "folder.svg",
    "layers.svg", "minimize-2.svg", "moon.svg", "repeat.svg", "search.svg", "sun.svg", "triangle-alert.svg", "x.svg", "zap.svg",
);

/// Serves the bundled icons to gpui's `svg()` element.
struct Assets;

impl gpui::AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        Ok(ICONS.iter().find(|(p, _)| *p == path).map(|(_, b)| std::borrow::Cow::Borrowed(*b)))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<gpui::SharedString>> {
        Ok(ICONS.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| gpui::SharedString::from(*p)).collect())
    }
}

gpui::actions!(react_graph, [Quit]);

static DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn dark() -> bool {
    DARK.load(std::sync::atomic::Ordering::Relaxed)
}

fn set_dark(on: bool) {
    DARK.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Every UI colour goes through here (it shadows `gpui::rgb`), so dark mode is one mapping
/// from the light palette instead of a second copy of every colour.
fn rgb(c: u32) -> gpui::Rgba {
    gpui::rgb(pal(c))
}

/// A light-palette colour as shown in the current mode.
fn pal(c: u32) -> u32 {
    if !dark() {
        return match c {
            CANVAS | ON_ACCENT => 0xffffff,
            _ => c,
        };
    }
    // JetBrains "Dark" surfaces and text for the colours with a fixed role
    match c {
        ON_ACCENT => 0xffffff,
        CANVAS | 0xfbfcfd | 0xf1f3f5 | 0xf0f0f0 => 0x1e1f22,
        0xffffff | 0xfefefe | 0xf8f9fb | TITLE_BG => 0x2b2d30,
        INK => 0xdfe1e5,
        0x1f2328 => 0xbcbec4,
        MUTED => 0x868a91,
        BORDER | 0xeeeeee | HOVER | 0xf4f4f4 | TITLE_HOVER => 0x393b40,
        SOFT_BORDER => 0x323438,
        _ => auto_dark(c),
    }
}

/// Dark-mode version of any other colour, keeping its hue: pale fills become dark tints,
/// dark text becomes light, vivid mid-tones (accent, kind colours) stay as they are.
fn auto_dark(c: u32) -> u32 {
    let (h, s, l) = to_hsl(c);
    let (s, l) = if l > 0.72 {
        (s * 0.45, 0.11 + (1.0 - l) * 0.9)
    } else if s > 0.3 && l >= 0.45 {
        (s, l)
    } else if s > 0.3 {
        (s, 1.0 - l)
    } else {
        (s, (1.08 - l).min(0.92))
    };
    from_hsl(h, s, l)
}

fn to_hsl(c: u32) -> (f32, f32, f32) {
    let ch = |shift: u32| ((c >> shift) & 0xff) as f32 / 255.0;
    let (r, g, b) = (ch(16), ch(8), ch(0));
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, s, l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> u32 {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h * 6.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let q = |v: f32| (((v + m).clamp(0.0, 1.0) * 255.0).round() as u32) & 0xff;
    (q(r) << 16) | (q(g) << 8) | q(b)
}

const INK: u32 = 0x1a192b;
const MUTED: u32 = 0x6b7280;
const BORDER: u32 = 0xe5e7eb;
const SOFT_BORDER: u32 = 0xeef2f6;
const DOT: u32 = 0x91919a;
const EDGE: u32 = 0xb1b1b7;
const HANDLE: u32 = 0x9ca3af;
// similar-code badge
const DUP_FG: u32 = 0x6d28d9;
const DUP_BG: u32 = 0xede9fe;
/// differing token in the compare view
const DUP_MARK: u32 = 0xddd6fe;
/// a return pulse in "Play flow"
const TEAL: u32 = 0x14b8a6;
/// how long the ring around a card grows after a pulse arrives (s)
const PING: f32 = 0.6;
/// flow player: width of the ‹ ▶ › speed buttons, and the gap before the progress track
const TRANSPORT_W: f32 = 124.0;
const TRACK_GAP: f32 = 10.0;
const CLOSE_W: f32 = 26.0;

// sequence diagram geometry (px)
const SEQ_COL: f32 = 172.0;
const SEQ_ROW: f32 = 38.0;
/// room on the left for row numbers
const SEQ_GUTTER: f32 = 56.0;
const SEQ_HEAD_Y: f32 = 48.0;
const SEQ_HEAD_H: f32 = 46.0;
/// top of the first row, below the lane headers
const SEQ_TOP: f32 = SEQ_HEAD_Y + SEQ_HEAD_H + 22.0;
/// where the arrow sits inside its row (the label goes above it)
const SEQ_ARROW: f32 = 27.0;
/// room kept free at the bottom for the player
const SEQ_BOTTOM: f32 = 210.0;

/// What is currently drawn (the expanded part of the graph, with its own layout).
#[derive(Default)]
struct ViewData {
    nodes: Vec<usize>,
    set: HashSet<usize>,
    edges: Vec<Edge>,
    pos: HashMap<usize, (f32, f32)>,
    size: (f32, f32),
    /// precomputed per relayout so render() stays O(visible nodes)
    has_in: HashSet<usize>,
    has_out: HashSet<usize>,
    kids: HashMap<usize, usize>,
}

struct GraphView {
    graph: Graph,
    root: String,
    selected: Option<usize>,
    /// fade every card that is not on the selected card's lineage
    focus_selection: bool,
    show_functions: bool,
    /// nodes whose children are revealed
    expanded: HashSet<usize>,
    /// extra nodes shown at top level (search hits not reachable from an entry point)
    pinned: Vec<usize>,
    entry_count: usize,
    isolated: usize,
    view: ViewData,
    zoom: f32,
    off: (f32, f32),
    drag: Option<(f32, f32)>,
    /// where the left button went down on the canvas, and its click count: a press that
    /// doesn't move is a click on whatever card is under it (zoomed out, cards are painted)
    press: Option<(f32, f32, usize)>,
    /// painted card under the mouse (zoomed out only)
    hover_card: Option<usize>,
    mm_drag: bool,
    fit_pending: bool,
    loading: bool,
    status: String,
    last_vp: (f32, f32),
    focus: FocusHandle,
    query: String,
    /// sidebar filters: narrow the search / list symbols that match, and fade the rest on the canvas
    filters: Filters,
    /// duplicate filter: group the list by clusters of similar code (else by file)
    filter_by_cluster: bool,
    /// filters only look at this symbol and everything below it (taken from the selection)
    filter_scope: Option<usize>,
    /// `filter_scope` and everything reachable from it
    scope_set: HashSet<usize>,
    /// selection the scope was last synced to
    scope_seen: Option<usize>,
    /// the next selection change came from the filter list: don't move the scope
    scope_hold: bool,
    /// file shown in the canvas (None = whole project from its entry points)
    focus_file: Option<String>,
    tree: DirNode,
    open_dirs: HashSet<String>,
    /// top-level symbol count per file
    file_syms: HashMap<String, usize>,
    /// absolute project folder (for reading source files)
    root_path: PathBuf,
    /// which tab the detail panel shows for a selected symbol
    source_tab: bool,
    /// show the focused file's source in the panel even with nothing selected
    file_panel: bool,
    code: CodeView,
    /// side-by-side compare of two similar symbols (replaces the detail panel while open)
    cmp: Option<CmpView>,
    /// "Play flow" trace from the selection; selecting anything else closes it
    flow: Option<FlowView>,
    /// player settings, kept for the next trace
    flow_from: Start,
    flow_style: Style,
    flow_speed: f32,
    /// show the flow as a sequence diagram instead of the graph
    flow_seq: bool,
    /// sequence diagram scroll (px)
    seq_scroll: (f32, f32),
    /// keep the playing row in view (off once the user scrolls the diagram)
    seq_follow: bool,
    /// right-click menu on a card: (card, window position)
    menu: Option<(usize, (f32, f32))>,
    /// theme and recent projects, saved between runs
    prefs: prefs::Prefs,
    /// open drop-down from the sidebar header
    popup: Option<Popup>,
    /// the popup a press outside just closed, and when: a press on its own toggle button
    /// closes it (mouse-down-out) and must not reopen it on the click that follows
    popup_closed: Option<(Popup, Instant)>,
    /// watches the open folder; dropping it ends the watch (and the task polling it)
    watcher: Option<notify::RecommendedWatcher>,
    /// a rebuild after a file change is running
    refreshing: bool,
    /// files changed again while loading / rebuilding: rebuild once more when done
    reload_pending: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Popup {
    /// the project widget in the title bar: open a folder, recent projects
    Project,
}

/// Which symbols the sidebar filters keep. Property filters combine with AND; kind filters
/// with OR (none picked = every kind).
#[derive(Default, Clone, Copy, PartialEq)]
struct Filters {
    /// has similar code elsewhere (`Symbol::similar`)
    dup: bool,
    /// has an identical-structure twin (only names / literals differ)
    exact: bool,
    /// dependency warnings on its own deps or the hooks it calls
    warn: bool,
    /// nothing renders, calls or passes it (its own definition doesn't count)
    unused: bool,
    /// wrapped in memo / useCallback / useMemo
    memo: bool,
    /// part of a render / call loop (A renders B renders A)
    cycle: bool,
    /// Component, Hook, Function
    kinds: [bool; 3],
}

impl Filters {
    fn active(&self) -> bool {
        self.dup || self.exact || self.warn || self.unused || self.memo || self.cycle || self.kinds.iter().any(|&k| k)
    }
}

fn kind_index(k: SymKind) -> usize {
    match k {
        SymKind::Component => 0,
        SymKind::Hook => 1,
        SymKind::Function => 2,
    }
}

/// An animated trace through the graph (see `flow`).
struct FlowView {
    trace: Trace,
    /// playback position (s)
    t: f32,
    playing: bool,
    /// when the last frame was drawn while playing
    last: Option<Instant>,
}

/// Open side-by-side compare.
struct CmpView {
    /// the selection it was opened from; selecting anything else closes it
    from: usize,
    data: Result<Rc<Compare>, String>,
    show_local: bool,
    /// difference picked in the list: its occurrences are emphasized
    focus: Option<usize>,
    scroll: UniformListScrollHandle,
    /// horizontal scroll shared by both sides (px)
    hscroll: f32,
}

/// Advance of one Consolas character at the code font size (12.5px).
const CHAR_W: f32 = 6.87;
/// Width of the "what differs" column in the compare panel.
const DIFFS_W: f32 = 270.0;

/// Source file shown in the code panel (cached between frames).
#[derive(Default)]
struct CodeView {
    file: Option<String>,
    src: Option<Result<Rc<SourceFile>, String>>,
    scroll: UniformListScrollHandle,
    /// what the list was last scrolled to (file, first line), so we only scroll on change
    scrolled_for: Option<(String, usize)>,
}

const ROW_H: f32 = 20.0;

// ---------------------------------------------------------------- file tree

#[derive(Default)]
struct DirNode {
    dirs: BTreeMap<String, DirNode>,
    /// full relative paths
    files: Vec<String>,
}

impl DirNode {
    fn build(files: &[String]) -> Self {
        let mut root = DirNode::default();
        for f in files {
            let parts: Vec<&str> = f.split('/').collect();
            let mut node = &mut root;
            for d in &parts[..parts.len() - 1] {
                node = node.dirs.entry(d.to_string()).or_default();
            }
            node.files.push(f.clone());
        }
        root.sort();
        root
    }

    fn sort(&mut self) {
        self.files.sort_by_key(|f| short_file(f).to_lowercase());
        for d in self.dirs.values_mut() {
            d.sort();
        }
    }
}

struct TreeRow {
    depth: usize,
    name: String,
    path: String,
    dir: bool,
    open: bool,
}

/// Flatten the visible part of the tree (folders first, VS Code-style compacted single-child folders).
/// `open = None` means "everything open".
fn tree_rows(node: &DirNode, prefix: &str, depth: usize, open: Option<&HashSet<String>>, out: &mut Vec<TreeRow>) {
    for (name, d) in &node.dirs {
        let mut label = name.clone();
        let mut path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let mut cur = d;
        while cur.files.is_empty() && cur.dirs.len() == 1 {
            let (n2, d2) = cur.dirs.iter().next().unwrap();
            label = format!("{label}/{n2}");
            path = format!("{path}/{n2}");
            cur = d2;
        }
        let is_open = open.is_none_or(|s| s.contains(&path));
        out.push(TreeRow { depth, name: label, path: path.clone(), dir: true, open: is_open });
        if is_open {
            tree_rows(cur, &path, depth + 1, open, out);
        }
    }
    for f in &node.files {
        out.push(TreeRow { depth, name: short_file(f).to_string(), path: f.clone(), dir: false, open: false });
    }
}

fn file_badge(path: &str) -> (&'static str, u32) {
    match path.rsplit('.').next().unwrap_or("") {
        "tsx" => ("TSX", 0x0ea5e9),
        "jsx" => ("JSX", 0x0ea5e9),
        "ts" => ("TS", 0x2563eb),
        "js" => ("JS", 0xca8a04),
        _ => ("•", MUTED),
    }
}

// ---------------------------------------------------------------- layout

fn layout(nodes: &[usize], edges: &[Edge], g: &Graph) -> (HashMap<usize, (f32, f32)>, (f32, f32)) {
    let n = nodes.len();
    if n == 0 {
        return (HashMap::new(), (0.0, 0.0));
    }
    let idx: HashMap<usize, usize> = nodes.iter().enumerate().map(|(k, &v)| (v, k)).collect();
    let mut succ = vec![Vec::new(); n];
    let mut pred = vec![Vec::new(); n];
    for e in edges {
        if let (Some(&a), Some(&b)) = (idx.get(&e.from), idx.get(&e.to)) {
            succ[a].push(b);
            pred[b].push(a);
        }
    }

    // DFS to find back edges (cycles), then longest-path layering.
    let mut color = vec![0u8; n];
    let mut back: HashSet<(usize, usize)> = HashSet::new();
    let mut post: Vec<usize> = Vec::with_capacity(n);
    let mut starts: Vec<usize> = (0..n).filter(|&k| pred[k].is_empty()).collect();
    starts.extend((0..n).filter(|&k| !pred[k].is_empty()));
    for s in starts {
        if color[s] != 0 {
            continue;
        }
        let mut stack = vec![(s, 0usize)];
        color[s] = 1;
        while let Some(&(u, ci)) = stack.last() {
            if ci < succ[u].len() {
                stack.last_mut().unwrap().1 += 1;
                let v = succ[u][ci];
                match color[v] {
                    0 => {
                        color[v] = 1;
                        stack.push((v, 0));
                    }
                    1 => {
                        back.insert((u, v));
                    }
                    _ => {}
                }
            } else {
                color[u] = 2;
                post.push(u);
                stack.pop();
            }
        }
    }
    let mut depth = vec![0usize; n];
    for &u in post.iter().rev() {
        for &v in &succ[u] {
            if !back.contains(&(u, v)) && depth[v] < depth[u] + 1 {
                depth[v] = depth[u] + 1;
            }
        }
    }

    let cols = depth.iter().copied().max().unwrap_or(0) + 1;
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); cols];
    for k in 0..n {
        columns[depth[k]].push(k);
    }
    for c in columns.iter_mut() {
        c.sort_by(|&a, &b| {
            let (sa, sb) = (&g.symbols[nodes[a]], &g.symbols[nodes[b]]);
            (&sa.file, sa.line).cmp(&(&sb.file, sb.line))
        });
    }

    // barycenter sweeps to reduce crossings
    let mut rank = vec![0.0f32; n];
    let update_rank = |columns: &Vec<Vec<usize>>, rank: &mut Vec<f32>| {
        for c in columns {
            for (r, &k) in c.iter().enumerate() {
                rank[k] = (r as f32 + 0.5) / c.len() as f32;
            }
        }
    };
    update_rank(&columns, &mut rank);
    for _ in 0..8 {
        for pass in 0..2 {
            let order: Vec<usize> = if pass == 0 {
                (1..cols).collect()
            } else {
                (0..cols.saturating_sub(1)).rev().collect()
            };
            for ci in order {
                let adj = if pass == 0 { &pred } else { &succ };
                let mut keyed: Vec<(f32, usize)> = columns[ci]
                    .iter()
                    .map(|&k| {
                        let a = &adj[k];
                        let b = if a.is_empty() {
                            rank[k]
                        } else {
                            a.iter().map(|&x| rank[x]).sum::<f32>() / a.len() as f32
                        };
                        (b, k)
                    })
                    .collect();
                keyed.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
                columns[ci] = keyed.into_iter().map(|(_, k)| k).collect();
                update_rank(&columns, &mut rank);
            }
        }
    }

    const MAX_ROWS: usize = 24;
    const SUB_GAP: f32 = 40.0;
    let per_of = |len: usize| {
        let k = len.div_ceil(MAX_ROWS).max(1);
        (k, len.div_ceil(k).max(1))
    };
    let tallest = columns.iter().map(|c| per_of(c.len()).1).max().unwrap_or(1);
    let height = tallest as f32 * (NODE_H + ROW_GAP) - ROW_GAP;
    let mut pos = HashMap::new();
    let mut x = 0.0f32;
    for c in columns.iter() {
        let (k, per) = per_of(c.len());
        for sub in 0..k {
            let slice = &c[(sub * per).min(c.len())..((sub + 1) * per).min(c.len())];
            let col_h = slice.len() as f32 * (NODE_H + ROW_GAP) - ROW_GAP;
            let y0 = (height - col_h) / 2.0;
            for (r, &kk) in slice.iter().enumerate() {
                pos.insert(nodes[kk], (x, y0 + r as f32 * (NODE_H + ROW_GAP)));
            }
            x += NODE_W + if sub + 1 < k { SUB_GAP } else { COL_GAP };
        }
    }
    let width = (x - COL_GAP).max(NODE_W);
    (pos, (width, height))
}

/// Open `file` at `line` with the `code` command (VS Code, and Cursor / VSCodium when they
/// provide one). False when there is no such command or it failed.
fn open_with_code(file: &std::path::Path, line: usize) -> bool {
    let target = format!("{}:{line}", file.display());
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        // `code` is a .cmd script there; no console window for it
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "code", "-g", &target]).creation_flags(0x0800_0000);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = std::process::Command::new("code");
        c.args(["-g", &target]);
        c
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Watches `root` recursively; the receiver gets a message for each change to a file that
/// affects the graph (see `graph::affects_graph`). Dropping the watcher closes the channel.
fn watch_folder(root: &std::path::Path) -> Option<(notify::RecommendedWatcher, std::sync::mpsc::Receiver<()>)> {
    use notify::Watcher;
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if !ev.kind.is_access() && ev.paths.iter().any(|p| graph::affects_graph(p)) {
                tx.send(()).ok();
            }
        }
    })
    .ok()?;
    w.watch(root, notify::RecursiveMode::Recursive).ok()?;
    Some((w, rx))
}

// ---------------------------------------------------------------- state

impl GraphView {
    fn empty(cx: &mut Context<Self>) -> Self {
        Self {
            graph: Graph::default(),
            root: String::new(),
            selected: None,
            focus_selection: true,
            show_functions: false,
            expanded: HashSet::new(),
            pinned: Vec::new(),
            entry_count: 0,
            isolated: 0,
            view: ViewData::default(),
            zoom: 1.0,
            off: (20.0, 20.0),
            drag: None,
            press: None,
            hover_card: None,
            mm_drag: false,
            fit_pending: false,
            loading: false,
            status: String::new(),
            last_vp: (0.0, 0.0),
            focus: cx.focus_handle(),
            query: String::new(),
            filters: Filters::default(),
            filter_by_cluster: true,
            filter_scope: None,
            scope_set: HashSet::new(),
            scope_seen: None,
            scope_hold: false,
            focus_file: None,
            tree: DirNode::default(),
            open_dirs: HashSet::new(),
            file_syms: HashMap::new(),
            root_path: PathBuf::new(),
            source_tab: false,
            file_panel: false,
            code: CodeView::default(),
            cmp: None,
            flow: None,
            flow_from: Start::Here,
            flow_style: Style::CallReturn,
            flow_speed: 1.0,
            flow_seq: false,
            seq_scroll: (0.0, 0.0),
            seq_follow: true,
            menu: None,
            prefs: prefs::Prefs::load(),
            popup: None,
            popup_closed: None,
            watcher: None,
            refreshing: false,
            reload_pending: false,
        }
    }

    /// Light or dark from the saved choice, else from the OS.
    fn init_theme(&mut self, system_dark: bool) {
        set_dark(match self.prefs.theme.as_str() {
            "dark" => true,
            "light" => false,
            _ => system_dark,
        });
    }

    fn toggle_theme(&mut self) {
        set_dark(!dark());
        self.prefs.theme = if dark() { "dark" } else { "light" }.into();
        self.prefs.save();
    }

    fn forget_recent(&mut self, p: &std::path::Path) {
        self.prefs.remove_recent(p);
        self.prefs.save();
    }

    fn clear_recent(&mut self) {
        self.prefs.recent.clear();
        self.prefs.save();
    }

    /// Ask for a folder and load it.
    fn prompt_open(cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(p) = paths.into_iter().next() {
                    this.update(cx, |this, cx| this.load_folder(p, cx)).ok();
                }
            }
        })
        .detach();
    }

    /// The drawn cards and links, for export.
    fn drawn(&self) -> export::Drawn<'_> {
        export::Drawn {
            graph: &self.graph,
            nodes: &self.view.nodes,
            edges: &self.view.edges,
            pos: &self.view.pos,
            card: (NODE_W, NODE_H),
        }
    }

    /// Save the drawn graph as an SVG picture, where the user chooses.
    fn export_svg(&mut self, cx: &mut Context<Self>) {
        let svg = export::svg(&self.drawn());
        let rx = cx.prompt_for_new_path(&self.root_path, Some("react-graph.svg"));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let msg = match std::fs::write(&path, svg) {
                    Ok(()) => format!("Saved {}", prefs::display(&path)),
                    Err(e) => format!("Could not save {}: {e}", prefs::display(&path)),
                };
                this.update(cx, |this, cx| {
                    this.status = msg;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Open the symbol's file at its line in VS Code; without the `code` command, in whatever
    /// the system opens that kind of file with.
    fn open_in_editor(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(s) = self.graph.symbols.get(i) else { return };
        let (path, line) = (self.root_path.join(&s.file), s.line);
        let (p, task) = (path.clone(), cx.background_spawn(async move { open_with_code(&path, line) }));
        cx.spawn(async move |this, cx| {
            if !task.await {
                this.update(cx, |this, cx| {
                    this.status = "No `code` command found: opened with the system default instead".into();
                    cx.open_with_system(&p);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Put the drawn graph on the clipboard as a Mermaid flowchart.
    fn copy_mermaid(&mut self, cx: &mut Context<Self>) {
        let text = export::mermaid(&self.drawn());
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.status = format!("Copied a Mermaid diagram of {} cards to the clipboard", self.view.nodes.len());
    }

    /// Open a folder from the recent list; one that is gone is dropped from the list instead.
    fn open_recent(&mut self, p: PathBuf, cx: &mut Context<Self>) {
        self.popup = None;
        if p.is_dir() {
            self.load_folder(p, cx);
        } else {
            self.status = format!("Folder not found: {}", prefs::display(&p));
            self.forget_recent(&p);
        }
        cx.notify();
    }

    /// Trace the flow from the selection with the player's settings, and play it.
    fn start_flow(&mut self) {
        let Some(sel) = self.selected else { return };
        // every kind of link is drawn, so the trace may follow all of them
        let trace = Trace::build(&self.graph, sel, self.flow_from, self.flow_style, &|_| true);
        // draw everything it touches: open what it goes through, pin what is still missing
        for s in trace.steps.iter().filter(|s| s.mv == Move::Go) {
            self.expanded.insert(s.edge.from);
        }
        self.relayout_keep(Some(sel));
        let mut missing: Vec<usize> = trace.nodes.iter().copied().filter(|n| !self.view.set.contains(n)).collect();
        if !missing.is_empty() {
            missing.sort();
            for n in missing {
                if !self.pinned.contains(&n) {
                    self.pinned.push(n);
                }
            }
            self.relayout_keep(Some(sel));
        }
        self.flow = Some(FlowView { trace, t: 0.0, playing: true, last: None });
        self.seq_scroll = (0.0, 0.0);
        self.seq_follow = true;
    }

    fn flow_toggle(&mut self) {
        self.seq_follow = true;
        let Some(f) = self.flow.as_mut() else { return };
        if f.playing {
            f.playing = false;
        } else {
            if f.t >= f.trace.total {
                f.t = 0.0;
            }
            f.playing = true;
            f.last = None;
        }
    }

    fn flow_seek(&mut self, t: f32) {
        self.seq_follow = true;
        if let Some(f) = self.flow.as_mut() {
            f.t = t.clamp(0.0, f.trace.total);
            f.playing = false;
        }
    }

    /// Jump to the end of the next / previous step and pause.
    fn flow_step(&mut self, forward: bool) {
        self.seq_follow = true;
        if let Some(f) = self.flow.as_mut() {
            f.t = if forward { f.trace.next_stop(f.t) } else { f.trace.prev_stop(f.t) };
            f.playing = false;
        }
    }

    /// Is the sequence diagram showing (instead of the graph)?
    fn seq_on(&self) -> bool {
        self.flow_seq && self.flow.is_some()
    }

    /// Width of the sequence diagram: between the sidebar and the panel on the right.
    fn seq_w(&self, vw: f32) -> f32 {
        (vw - LEFT_W - self.right_gap(vw)).max(240.0)
    }

    /// Keep the sequence diagram's scroll in range, and the row playing now in view
    /// (gliding while it plays, jumping after a step or seek).
    fn seq_scroll_update(&mut self, vw: f32, vh: f32) {
        let w = self.seq_w(vw);
        let Some(f) = self.flow.as_ref() else { return };
        let seq = &f.trace.seq;
        let content_w = SEQ_GUTTER + seq.lanes.len() as f32 * SEQ_COL + 40.0;
        let content_h = SEQ_TOP + seq.rows.len() as f32 * SEQ_ROW + SEQ_BOTTOM;
        let (max_x, max_y) = ((content_w - w).max(0.0), (content_h - vh).max(0.0));
        let mut s = self.seq_scroll;
        let row = f.trace.frame(f.t).current.and_then(|i| seq.row_of(i));
        if let (Some(r), true) = (row, self.seq_follow) {
            let y = SEQ_TOP + r as f32 * SEQ_ROW;
            let mut ty = s.1;
            if y - ty > vh - SEQ_BOTTOM - SEQ_ROW {
                ty = y - (vh - SEQ_BOTTOM - SEQ_ROW);
            }
            if y - ty < SEQ_TOP {
                ty = y - SEQ_TOP;
            }
            let lane_x = |l: usize| SEQ_GUTTER + (l as f32 + 0.5) * SEQ_COL;
            // the arrow's lanes (with their headers) in view; too wide: centre where it arrives
            let (a, b) = (lane_x(seq.rows[r].from), lane_x(seq.rows[r].to));
            let (lo, hi) = (a.min(b) - SEQ_COL * 0.5, a.max(b) + SEQ_COL * 0.5);
            let mut tx = s.0;
            if hi - lo <= w - SEQ_GUTTER {
                if hi - tx > w {
                    tx = hi - w;
                }
                if lo - tx < SEQ_GUTTER {
                    tx = lo - SEQ_GUTTER;
                }
            } else {
                tx = b - (SEQ_GUTTER + w) / 2.0;
            }
            let k = if f.playing { 0.15 } else { 1.0 };
            s = (s.0 + (tx - s.0) * k, s.1 + (ty - s.1) * k);
        }
        self.seq_scroll = (s.0.clamp(0.0, max_x), s.1.clamp(0.0, max_y));
    }

    /// The flow as a sequence diagram: a lifeline per symbol, an arrow per step (solid = call or
    /// render, dashed = return), activation bars for what is on the stack. Drawn as it plays.
    fn sequence_view(&self, vw: f32, vh: f32, fr: &Frame, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(fv) = self.flow.as_ref() else { return div().into_any_element() };
        let (tr, t) = (&fv.trace, fv.t);
        let seq = &tr.seq;
        let w = self.seq_w(vw);
        let (sx, sy) = self.seq_scroll;
        let lane_x = move |l: usize| SEQ_GUTTER + (l as f32 + 0.5) * SEQ_COL - sx;
        let row_y = move |r: usize| SEQ_TOP + r as f32 * SEQ_ROW - sy;
        let n = seq.rows.len();
        // rows on screen (the ones scrolled under the headers are covered)
        let first = ((sy / SEQ_ROW).floor() as usize).saturating_sub(1).min(n);
        let last = (((sy + vh) / SEQ_ROW).ceil() as usize + 1).min(n);
        let cur_row = fr.current.and_then(|i| seq.row_of(i));
        // (progress, done): progress < 0 = not started yet
        let state = |r: usize| {
            let s = &tr.steps[seq.rows[r].step];
            ((t - s.t0) / s.dur, t >= s.t0 + s.dur)
        };
        let waves = tr.style == Style::Waves;
        let running = |sym: usize| {
            fr.stack.last() == Some(&sym) || (waves && fr.pings.get(&sym).is_some_and(|&d| (0.0..PING).contains(&d)))
        };

        // activation bars: for every row already played, what is on the stack until the next row
        let mut bars: Vec<SeqBar> = Vec::new();
        let mut push_bars = |stack: &[usize], y0: f32, y1: f32| {
            for (k, &l) in stack.iter().enumerate() {
                // the same lane lower in the stack (recursion): nest the bar a little to the right
                let level = stack[..k].iter().filter(|&&x| x == l).count();
                bars.push((lane_x(l) - 5.0 + level as f32 * 5.0, y0, y1, k + 1 == stack.len()));
            }
        };
        if !waves {
            let end0 = if n > 0 { row_y(0) + SEQ_ARROW } else { SEQ_TOP + 30.0 - sy };
            push_bars(&seq.stack0, SEQ_TOP - 12.0 - sy, end0);
            for r in first..last {
                if !state(r).1 {
                    break;
                }
                let y0 = row_y(r) + SEQ_ARROW;
                let y1 = if r + 1 < n { row_y(r + 1) + SEQ_ARROW } else { y0 + SEQ_ROW * 0.6 };
                push_bars(&seq.rows[r].stack, y0, y1);
            }
        }

        // arrows and their labels
        let mut arrows: Vec<SeqArrow> = Vec::new();
        let mut rows_el: Vec<gpui::AnyElement> = Vec::new();
        for r in first..last {
            let row = &seq.rows[r];
            let s = &tr.steps[row.step];
            let (p, done) = state(r);
            let (started, back) = (p >= 0.0, s.mv == Move::Back);
            let playing = started && !done;
            let (xa, xb) = (lane_x(row.from), lane_x(row.to));
            let dir = if xb >= xa { 1.0 } else { -1.0 };
            let base = if back {
                TEAL
            } else if s.recursion {
                0xb45309
            } else if s.edge.kind == EdgeKind::Handler {
                0x16a34a
            } else {
                0x475569
            };
            let moving = if back { TEAL } else { ACCENT };
            arrows.push(SeqArrow {
                a: xa + dir * 6.0,
                b: xb - dir * 6.0,
                y: row_y(r) + SEQ_ARROW,
                color: if playing { moving } else { base },
                alpha: if started { 1.0 } else { 0.16 },
                dashed: back,
                q: if playing { flow::ease(p.clamp(0.0, 1.0)) } else { 1.0 },
                head: !playing,
                dot: playing.then_some((moving, s.later)),
            });
            let y = row_y(r);
            if y < SEQ_TOP - SEQ_ROW * 0.5 {
                continue;
            }
            let to = &self.graph.symbols[s.edge.to].name;
            let label = match (s.mv, s.edge.kind) {
                (Move::Back, _) => "return".to_string(),
                (Move::Go, _) if s.recursion => format!("{to}() ↺"),
                (Move::Go, EdgeKind::Calls) => format!("{to}()"),
                (Move::Go, EdgeKind::Renders) => format!("<{to} />"),
                (Move::Go, EdgeKind::Handler) => format!("on event: {to}()"),
                (Move::Go, EdgeKind::Defines) => format!("defines {to}"),
                (Move::Go, EdgeKind::Context) => format!("context to {to}"),
            };
            let cur = cur_row == Some(r);
            let color = if cur {
                ACCENT
            } else if !started {
                0xcbd5e1
            } else if back {
                0x0f766e
            } else {
                0x334155
            };
            let end = s.t0 + s.dur;
            // label centred over the part of the arrow that is on screen (none if it is all off screen)
            let visible = xa.max(xb) > SEQ_GUTTER + 24.0 && xa.min(xb) < w - 24.0;
            let lab_l = xa.min(xb).max(SEQ_GUTTER - 8.0) + 8.0;
            let lab_w = (xa.max(xb).min(w) - 8.0 - lab_l).max(60.0);
            rows_el.push(
                div()
                    .id(("seq-row", r))
                    .absolute()
                    .left_0()
                    .top(px(y))
                    .w(px(w))
                    .h(px(SEQ_ROW))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.flow_seek(end);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(SEQ_ARROW - 8.0))
                            .w(px(SEQ_GUTTER - 16.0))
                            .text_right()
                            .text_xs()
                            .text_color(rgb(if cur { ACCENT } else { 0x9ca3af }))
                            .child((r + 1).to_string()),
                    )
                    .when(visible, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left(px(lab_l))
                                .top(px(5.0))
                                .w(px(lab_w))
                                .text_center()
                                .text_xs()
                                .truncate()
                                .font_family(MONO_FONT)
                                .text_color(rgb(color))
                                .when(cur, |d| d.font_weight(gpui::FontWeight::BOLD))
                                .child(label),
                        )
                    })
                    .into_any_element(),
            );
        }

        // lane headers, kept at the top while the rows scroll
        let mut heads: Vec<gpui::AnyElement> = Vec::new();
        let mut lanes_x: Vec<f32> = Vec::new();
        for (l, &sym) in seq.lanes.iter().enumerate() {
            let x = lane_x(l);
            if x < -SEQ_COL || x > w + SEQ_COL {
                continue;
            }
            lanes_x.push(x);
            let s = &self.graph.symbols[sym];
            let (reached, active) = (fr.reached.contains(&sym), running(sym));
            heads.push(
                div()
                    .absolute()
                    .left(px(x - SEQ_COL / 2.0 + 10.0))
                    .top(px(SEQ_HEAD_Y))
                    .w(px(SEQ_COL - 20.0))
                    .h(px(SEQ_HEAD_H))
                    .px_2()
                    .rounded(px(10.))
                    .bg(flow_fill(s.kind, reached, active).unwrap_or(rgb(0xffffff)))
                    .border_1()
                    .when(active, |d| d.border_2())
                    .border_color(rgb(if reached { ACCENT } else { SOFT_BORDER }))
                    .shadow_sm()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .min_w(px(0.))
                            .child(div().flex_shrink_0().w(px(7.)).h(px(7.)).rounded_full().bg(kind_color(s.kind)))
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(INK))
                                    .child(s.name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("{} · {}", kind_name(s.kind), short_file(&s.file))),
                    )
                    .into_any_element(),
            );
        }

        let band = cur_row.map(row_y);
        let life_end = (row_y(n) + 12.0).min(vh);
        let paint = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let at = |x: f32, y: f32| bounds.origin + point(px(x), px(y));
                let stroke = |window: &mut Window, (x0, y0): Pt, (x1, y1): Pt, width: f32, color: Hsla, dashed: bool| {
                    let mut pb = PathBuilder::stroke(px(width));
                    if dashed {
                        pb = pb.dash_array(&[px(5.0), px(4.0)]);
                    }
                    pb.move_to(at(x0, y0));
                    pb.line_to(at(x1, y1));
                    if let Ok(path) = pb.build() {
                        window.paint_path(path, color);
                    }
                };
                // the row playing now
                if let Some(y) = band {
                    window.paint_quad(fill(Bounds::new(at(0.0, y), size(px(w), px(SEQ_ROW))), rgb(ACCENT_50)));
                }
                // lifelines
                for &x in &lanes_x {
                    stroke(window, (x, SEQ_TOP - 12.0), (x, life_end), 1.0, hsla(0xcbd5e1, 1.0), true);
                }
                // activation bars (left and right edges only, so stacked segments join cleanly)
                for &(x, y0, y1, top) in &bars {
                    window.paint_quad(
                        fill(Bounds::new(at(x, y0), size(px(10.0), px((y1 - y0).max(0.0)))), rgb(if top { ACCENT_200 } else { 0xe2e8f0 }))
                            .border_widths(gpui::Edges { top: px(0.0), right: px(1.0), bottom: px(0.0), left: px(1.0) })
                            .border_color(rgb(if top { ACCENT_400 } else { 0x94a3b8 })),
                    );
                }
                // arrows
                for ar in &arrows {
                    let c = hsla(ar.color, ar.alpha);
                    let tip = ar.a + (ar.b - ar.a) * ar.q;
                    stroke(window, (ar.a, ar.y), (tip, ar.y), if ar.dot.is_some() { 2.2 } else { 1.5 }, c, ar.dashed);
                    if ar.head {
                        let d = if ar.b >= ar.a { 1.0 } else { -1.0 };
                        let mut pb = PathBuilder::fill();
                        pb.move_to(at(ar.b, ar.y));
                        pb.line_to(at(ar.b - d * 8.0, ar.y - 4.0));
                        pb.line_to(at(ar.b - d * 8.0, ar.y + 4.0));
                        pb.close();
                        if let Ok(path) = pb.build() {
                            window.paint_path(path, c);
                        }
                    }
                    if let Some((color, hollow)) = ar.dot {
                        let dot = |r: f32| Bounds::new(at(tip - r, ar.y - r), size(px(2.0 * r), px(2.0 * r)));
                        window.paint_quad(fill(dot(9.0), hsla(color, 0.18)).corner_radii(px(9.0)));
                        window.paint_quad(fill(dot(4.5), hsla(color, 1.0)).corner_radii(px(4.5)));
                        if hollow {
                            window.paint_quad(fill(dot(2.2), hsla(0xffffff, 1.0)).corner_radii(px(2.2)));
                        }
                    }
                }
                // the row numbers stay on the left: cover what is scrolled under them
                window.paint_quad(fill(
                    Bounds::new(at(0.0, 0.0), size(px(SEQ_GUTTER - 10.0), px(vh))),
                    hsla(0xffffff, 1.0),
                ));
                if let Some(y) = band {
                    window.paint_quad(fill(Bounds::new(at(0.0, y), size(px(SEQ_GUTTER - 10.0), px(SEQ_ROW))), rgb(ACCENT_50)));
                }
                // cover the rows scrolled up under the headers, keep the lifelines going up to them
                window.paint_quad(fill(Bounds::new(at(0.0, 0.0), size(px(w), px(SEQ_TOP - 10.0))), rgb(CANVAS)));
                for &x in &lanes_x {
                    stroke(window, (x, SEQ_HEAD_Y + SEQ_HEAD_H), (x, SEQ_TOP - 10.0), 1.0, hsla(0xcbd5e1, 1.0), true);
                }
            },
        )
        .absolute()
        .size_full();

        let start = &self.graph.symbols[tr.start].name;
        let what = match tr.style {
            Style::CallReturn => "call and return",
            Style::Waves => "waves",
        };
        div()
            .id("seq")
            .occlude()
            .absolute()
            .left(px(LEFT_W))
            .top_0()
            .w(px(w))
            .h(px(vh))
            .bg(rgb(CANVAS))
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                let d = ev.delta.pixel_delta(px(20.));
                let (dx, dy) = (f32::from(d.x), f32::from(d.y));
                // shift + wheel scrolls sideways (Windows already sends it as a sideways delta;
                // swap only when it still arrives as vertical, or it would flip back)
                let (dx, dy) = if ev.modifiers.shift && dx == 0.0 { (dy, 0.0) } else { (dx, dy) };
                this.seq_scroll = (this.seq_scroll.0 - dx, this.seq_scroll.1 - dy);
                this.seq_follow = false;
                cx.notify();
            }))
            .child(paint)
            .children(rows_el)
            .children(heads)
            .child(
                div()
                    .absolute()
                    .left(px(SEQ_GUTTER - 24.0))
                    .top(px(14.))
                    .w(px(w - SEQ_GUTTER))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div().flex_shrink_0().text_sm().font_weight(gpui::FontWeight::BOLD).text_color(rgb(INK)).child("Sequence"),
                    )
                    .child(div().min_w(px(0.)).truncate().text_xs().text_color(rgb(MUTED)).child(format!(
                        "{what} from {start} · {n} steps · dashed = return · shift + scroll pans sideways"
                    ))),
            )
            .into_any_element()
    }

    /// Show `a` and `b` side by side (keeps the current selection).
    fn open_compare(&mut self, a: usize, b: usize) {
        let from = self.selected.unwrap_or(a);
        let show_local = self.cmp.as_ref().is_some_and(|c| c.show_local);
        let data = compare::build(&self.graph, &self.root_path, a, b).map(Rc::new);
        self.cmp = Some(CmpView { from, data, show_local, focus: None, scroll: UniformListScrollHandle::new(), hscroll: 0.0 });
    }

    /// Width of one code side in the compare panel, without its gutter (px).
    fn cmp_text_w(&self, vw: f32) -> f32 {
        (self.cmp_w(vw) - 2.0 * 16.0 - DIFFS_W - 12.0 - 3.0) / 2.0 - 3.0 - 40.0 - 16.0
    }

    /// Scroll both sides horizontally by `dx`, within the longest shown line.
    fn cmp_hscroll(&mut self, dx: f32, vw: f32) {
        let text_w = self.cmp_text_w(vw);
        let Some(cv) = self.cmp.as_mut() else { return };
        let Ok(c) = &cv.data else { return };
        let len = |src: &SourceFile, l: Option<usize>| l.map_or(0, |l| src.lines[l].chars().count());
        let widest = c.rows.iter().map(|r| len(&c.src_a, r.a).max(len(&c.src_b, r.b))).max().unwrap_or(0);
        let max = (widest as f32 * CHAR_W - text_w + 24.0).max(0.0);
        cv.hscroll = (cv.hscroll + dx).clamp(0.0, max);
    }

    /// Pick a difference in the list: emphasize it, scroll so its first occurrence is in view.
    fn cmp_focus(&mut self, i: usize, vw: f32) {
        let text_w = self.cmp_text_w(vw);
        let Some(cv) = self.cmp.as_mut() else { return };
        let Ok(c) = cv.data.clone() else { return };
        cv.focus = if cv.focus == Some(i) { None } else { Some(i) };
        let p = &c.pairs[i];
        cv.scroll.scroll_to_item_strict(p.row.saturating_sub(3), ScrollStrategy::Top);
        // furthest-right start of this difference on the first row it appears in
        let r = c.rows[p.row];
        let col = |src: &SourceFile, marks: &HashMap<usize, Vec<compare::Mark>>, l: Option<usize>| {
            let l = l?;
            let m = marks.get(&l)?.iter().find(|m| m.pair == i)?;
            Some(src.lines[l][..m.range.start].chars().count() + src.lines[l][m.range.clone()].chars().count())
        };
        let end = col(&c.src_a, &c.marks_a, r.a).max(col(&c.src_b, &c.marks_b, r.b)).unwrap_or(0) as f32 * CHAR_W;
        let target = if end > text_w - 16.0 { end - text_w + 48.0 } else { 0.0 };
        cv.hscroll = 0.0;
        self.cmp_hscroll(target, vw);
    }

    /// Make sure `file` is loaded in the code panel; scroll so `line` (1-based) is near the top.
    fn ensure_code(&mut self, file: &str, line: usize) {
        if self.code.file.as_deref() != Some(file) {
            self.code.file = Some(file.to_string());
            self.code.src = Some(source::load(&self.root_path.join(file)).map(Rc::new));
            self.code.scrolled_for = None;
        }
        let key = (file.to_string(), line);
        if self.code.scrolled_for.as_ref() != Some(&key) {
            self.code.scroll.scroll_to_item_strict(line.saturating_sub(4), ScrollStrategy::Top);
            self.code.scrolled_for = Some(key);
        }
    }

    /// Innermost symbol of `file` whose declaration covers `line`.
    fn symbol_at(&self, file: &str, line: usize) -> Option<usize> {
        let g = &self.graph;
        (0..g.symbols.len())
            .filter(|&i| g.symbols[i].file == file && g.symbols[i].line <= line && line <= g.symbols[i].end_line)
            .min_by_key(|&i| g.symbols[i].end_line - g.symbols[i].line)
    }

    /// Is the panel on the right currently showing source code?
    fn showing_code(&self) -> bool {
        if self.selected.is_some() { self.source_tab } else { self.file_panel && self.focus_file.is_some() }
    }

    /// Clicking a line in the code panel selects the innermost component / hook / function there.
    fn select_line(&mut self, file: &str, line: usize) {
        let Some(i) = self.symbol_at(file, line) else { return };
        // the line is already on screen: don't jump the code view
        self.code.scrolled_for = Some((file.to_string(), self.graph.symbols[i].line));
        self.source_tab = true;
        self.reveal(i);
    }

    /// Syntax-highlighted, virtualized view of the loaded file; `range` (1-based, inclusive) is highlighted.
    fn code_block(&self, range: Option<(usize, usize)>, cx: &mut Context<Self>) -> gpui::AnyElement {
        let frame = div()
            .flex_1()
            .min_h(px(0.))
            .mt_2()
            .rounded(px(8.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(0xfbfcfd))
            .overflow_hidden()
            .flex()
            .flex_col();
        let src = match &self.code.src {
            Some(Ok(s)) => s.clone(),
            Some(Err(e)) => {
                return frame.p_3().text_sm().text_color(rgb(0xb91c1c)).child(e.clone()).into_any_element();
            }
            None => return frame.into_any_element(),
        };
        let file = self.code.file.clone().unwrap_or_default();
        let this = cx.entity().downgrade();
        let gutter = src.lines.len().to_string().len() as f32 * 8.0 + 18.0;
        let widest = src.widest;
        let list = uniform_list("code-lines", src.lines.len(), move |rows, _, _| {
            rows.map(|ix| {
                let ln = ix + 1;
                let hot = range.is_some_and(|(a, b)| a <= ln && ln <= b);
                let (this, file) = (this.clone(), file.clone());
                let text = if src.lines[ix].is_empty() { " ".into() } else { src.lines[ix].clone() };
                div()
                    .id(("code-line", ix))
                    .h(px(ROW_H))
                    .min_w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .whitespace_nowrap()
                    .font_family(MONO_FONT)
                    .text_size(px(12.5))
                    .text_color(rgb(0x1f2328))
                    .cursor_pointer()
                    .when(hot, |d| d.bg(rgb(ACCENT_50)))
                    .when(!hot, |d| d.hover(|s| s.bg(rgb(0xf3f4f6))))
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| {
                            this.select_line(&file, ln);
                            cx.notify();
                        })
                        .ok();
                    })
                    .child(div().w(px(3.)).h_full().flex_shrink_0().when(hot, |d| d.bg(rgb(ACCENT))))
                    .child(
                        div()
                            .w(px(gutter))
                            .flex_shrink_0()
                            .pr_3()
                            .text_right()
                            .text_color(rgb(if hot { ACCENT } else { 0xa0a7b1 }))
                            .child(ln.to_string()),
                    )
                    .child(StyledText::new(text).with_highlights(src.highlights[ix].clone()))
                    .child(div().w(px(16.)).flex_shrink_0())
            })
            .collect::<Vec<_>>()
        })
        .track_scroll(self.code.scroll.clone())
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(Some(widest))
        .flex_1()
        .min_h(px(0.))
        .py_1();
        frame.child(list).into_any_element()
    }

    /// Side-by-side compare of two similar symbols, with the list of what differs.
    fn compare_panel(&self, vw: f32, vh: f32, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let cv = self.cmp.as_ref()?;
        let frame = card()
            .id("compare")
            .occlude()
            .absolute()
            .top(px(MARGIN))
            .right(px(MARGIN))
            .w(px(self.cmp_w(vw)))
            .h(px(vh - 2.0 * MARGIN))
            .p_4()
            .flex()
            .flex_col()
            .gap_1();
        let top = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(div().text_xs().font_family(MONO_FONT).text_color(rgb(MUTED)).child("compare"))
            .child(
                div()
                    .id("close-cmp")
                    .px_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_color(rgb(MUTED))
                    .hover(|st| st.bg(rgb(0xf3f4f6)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cmp = None;
                        cx.notify();
                    }))
                    .child("✕"),
            );
        let c = match &cv.data {
            Ok(c) => c.clone(),
            Err(e) => {
                let err = div().mt_2().text_sm().text_color(rgb(0xb91c1c)).child(e.clone());
                return Some(frame.child(top).child(err).into_any_element());
            }
        };
        let (a, b) = (c.a, c.b);
        let (sa, sb) = (&self.graph.symbols[a], &self.graph.symbols[b]);

        // ---- title: A ↔ B, how alike, actions
        let badge = sa
            .similar
            .iter()
            .find(|m| m.other == b)
            .map(|m| if m.exact { "identical structure".to_string() } else { format!("{} similar", pct(m.score)) })
            .unwrap_or_default();
        let name = |s: &graph::Symbol| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(kind_color(s.kind)))
                .child(div().text_xl().font_weight(gpui::FontWeight::BOLD).child(s.name.clone()))
        };
        let title = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_3()
            .child(name(sa))
            .child(div().text_color(rgb(MUTED)).child("↔"))
            .child(name(sb))
            .child(
                div()
                    .px(px(6.))
                    .rounded(px(4.))
                    .bg(rgb(DUP_BG))
                    .text_xs()
                    .text_color(rgb(DUP_FG))
                    .child(badge),
            )
            .child(div().flex_1())
            .child(ghost_button("cmp-swap", "Swap sides").on_click(cx.listener(move |this, _, _, cx| {
                this.open_compare(b, a);
                cx.notify();
            })))
            .child(ghost_button("cmp-go", format!("Go to {}", sb.name)).on_click(cx.listener(move |this, _, _, cx| {
                this.cmp = None;
                this.reveal(b);
                cx.notify();
            })));

        let count = |k: DiffKind| c.pairs.iter().filter(|p| p.kind == k).count();
        let (params, code, locals) = (count(DiffKind::Name) + count(DiffKind::Literal), count(DiffKind::Code), count(DiffKind::Local));
        let summary = if !c.exact {
            let lines = |n: usize| if n == 1 { "1 line".to_string() } else { format!("{n} lines") };
            let what = match c.changed_lines() {
                (0, n) => format!("{} has {} more", sb.name, lines(n)),
                (n, 0) => format!("{} has {} more", sa.name, lines(n)),
                (x, y) => format!("{} of {} and {} of {} don't match", lines(x), sa.name, lines(y), sb.name),
            };
            format!("Similar, not identical: {what} (amber). Matching lines sit side by side.")
        } else if params + code == 0 {
            "Same code apart from local names: one could simply replace the other.".to_string()
        } else {
            "Same structure. The highlighted names and values are what a shared version would take as parameters. \
             Click one to find it; Shift+scroll for long lines."
                .to_string()
        };

        // ---- aligned code
        let gutter = |s: &graph::Symbol| s.end_line.to_string().len() as f32 * 8.0 + 14.0;
        let (ga, gb) = (gutter(sa), gutter(sb));
        let (focus, show_local, hs) = (cv.focus, cv.show_local, cv.hscroll);
        let rows = c.clone();
        let list = uniform_list("cmp-rows", c.rows.len(), move |range, _, _| {
            range
                .map(|ix| {
                    let r = rows.rows[ix];
                    let half = |src: &SourceFile, marks: &HashMap<usize, Vec<compare::Mark>>, line: Option<usize>, g: f32| {
                        cmp_half(src, marks, &rows.pairs, line, r.changed, g, focus, show_local, hs)
                    };
                    div()
                        .h(px(ROW_H))
                        .w_full()
                        .flex()
                        .flex_row()
                        .whitespace_nowrap()
                        .font_family(MONO_FONT)
                        .text_size(px(12.5))
                        .text_color(rgb(0x1f2328))
                        .child(half(&rows.src_a, &rows.marks_a, r.a, ga))
                        .child(div().w(px(1.)).h_full().flex_shrink_0().bg(rgb(BORDER)))
                        .child(half(&rows.src_b, &rows.marks_b, r.b, gb))
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(cv.scroll.clone())
        .flex_1()
        .min_h(px(0.))
        .py_1();
        let label = |s: &graph::Symbol| {
            div()
                .flex_1()
                .min_w(px(0.))
                .px_3()
                .py_1()
                .truncate()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(format!("{}:{}", s.file, s.line))
        };
        let code_frame = div()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .rounded(px(8.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(0xfbfcfd))
            .overflow_hidden()
            .flex()
            .flex_col()
            .id("cmp-code")
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, window, cx| {
                let d = ev.delta.pixel_delta(px(20.));
                let dx = if f32::from(d.x) != 0.0 {
                    -f32::from(d.x)
                } else if ev.modifiers.shift {
                    -f32::from(d.y)
                } else {
                    return;
                };
                this.cmp_hscroll(dx, f32::from(window.viewport_size().width));
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(label(sa))
                    .child(div().w(px(1.)).bg(rgb(BORDER)))
                    .child(label(sb)),
            )
            .child(list);

        // ---- what differs
        let mut side = div().id("cmp-diffs").w(px(DIFFS_W)).flex_shrink_0().h_full().overflow_y_scroll().flex().flex_col();
        let pair_row = |i: usize, cx: &mut Context<Self>| {
            let p = &c.pairs[i];
            div()
                .id(("cmp-pair", i))
                .px_2()
                .py(px(3.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(focus == Some(i), |d| d.bg(rgb(ACCENT_100)))
                .hover(|st| st.bg(rgb(0xf5f3ff)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.cmp_focus(i, f32::from(window.viewport_size().width));
                    cx.notify();
                }))
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_x_1()
                .text_xs()
                .font_family(MONO_FONT)
                .child(p.a.clone())
                .child(div().text_color(rgb(MUTED)).child("→"))
                .child(p.b.clone())
                .when(p.count > 1, |d| {
                    d.child(div().flex_1()).child(div().text_color(rgb(MUTED)).child(format!("×{}", p.count)))
                })
        };
        let groups = [
            (DiffKind::Name, "Would become parameters"),
            (DiffKind::Code, "Different nested code"),
        ];
        for (k, heading) in groups {
            let ix: Vec<usize> = (0..c.pairs.len())
                .filter(|&i| if k == DiffKind::Name { matches!(c.pairs[i].kind, DiffKind::Name | DiffKind::Literal) } else { c.pairs[i].kind == k })
                .collect();
            if ix.is_empty() {
                continue;
            }
            side = side.child(section(heading).mt_0());
            for i in ix {
                side = side.child(pair_row(i, cx));
            }
        }
        if c.exact && params + code == 0 {
            side = side.child(section("Would become parameters").mt_0()).child(
                div().px_2().text_xs().text_color(rgb(MUTED)).child("Nothing: the two differ only in local names."),
            );
        }
        if locals > 0 {
            side = side.child(
                div()
                    .mt_3()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(section(&format!("Renamed locals ({locals})")).mt_0())
                    .child(
                        div()
                            .id("cmp-locals")
                            .px_2()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(ACCENT))
                            .hover(|st| st.bg(rgb(ACCENT_50)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(cv) = this.cmp.as_mut() {
                                    cv.show_local = !cv.show_local;
                                    cv.focus = None;
                                }
                                cx.notify();
                            }))
                            .child(if show_local { "Hide" } else { "Show" }),
                    ),
            );
            if show_local {
                for i in (0..c.pairs.len()).filter(|&i| c.pairs[i].kind == DiffKind::Local) {
                    side = side.child(pair_row(i, cx));
                }
            } else {
                side = side.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("Variables and parameters named differently. They don't need to become parameters."),
                );
            }
        }

        Some(
            frame
                .child(top)
                .child(title)
                .child(div().text_xs().text_color(rgb(MUTED)).child(summary))
                .child(div().flex_1().min_h(px(0.)).mt_2().flex().flex_row().gap_3().child(code_frame).child(side))
                .into_any_element(),
        )
    }

    /// The compare panel takes everything right of the explorer.
    fn cmp_w(&self, vw: f32) -> f32 {
        (vw - LEFT_W - 3.0 * MARGIN).max(DETAIL_W)
    }

    fn code_w(&self, vw: f32) -> f32 {
        ((vw - LEFT_W - 2.0 * MARGIN) * 0.55).clamp(DETAIL_W, 900.0)
    }

    /// Width taken on the right by the detail / code panel (including margins).
    fn right_gap(&self, vw: f32) -> f32 {
        if self.cmp.is_some() {
            self.cmp_w(vw) + 2.0 * MARGIN
        } else if self.showing_code() {
            self.code_w(vw) + 2.0 * MARGIN
        } else if self.selected.is_some() {
            DETAIL_W + 2.0 * MARGIN
        } else {
            MARGIN
        }
    }

    /// Show one file in the canvas (or the whole project with `None`).
    fn open_file(&mut self, f: Option<String>) {
        if f.is_none() {
            self.file_panel = false;
        }
        self.focus_file = f;
        self.expanded.clear();
        self.pinned.clear();
        self.selected = None;
        if let Some(f) = self.focus_file.clone() {
            // reveal it in the explorer
            let mut acc = String::new();
            for seg in f.split('/').take(f.split('/').count().saturating_sub(1)) {
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(seg);
                self.open_dirs.insert(acc.clone());
            }
            // show what the file's symbols use right away
            let (_, roots, _) = self.roots();
            self.expanded.extend(roots);
        }
        self.relayout();
        self.fit_pending = true;
    }

    fn load_folder(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.loading = true;
        let shown = root.to_string_lossy().to_string();
        self.root = shown.strip_prefix(r"\\?\").unwrap_or(&shown).to_string();
        self.root_path = root.clone();
        self.code = CodeView::default();
        self.cmp = None;
        self.flow = None;
        self.file_panel = false;
        self.selected = None;
        self.scope_seen = None;
        self.scope_hold = false;
        self.set_scope(None);
        self.expanded.clear();
        self.pinned.clear();
        self.focus_file = None;
        self.tree = DirNode::default();
        self.view = ViewData::default();
        self.status = "Loading…".into();
        self.reload_pending = false;
        self.start_watching(cx);
        cx.notify();
        let task = cx.background_spawn(async move {
            let t = Instant::now();
            let g = graph::build(&root);
            (g, t.elapsed())
        });
        let root_done = self.root_path.clone();
        cx.spawn(async move |this, cx| {
            let (g, took) = task.await;
            this.update(cx, |this, cx| {
                this.status = format!(
                    "{} files · {} symbols · {} edges · {:.1}s",
                    g.files,
                    g.symbols.len(),
                    g.edges.len(),
                    took.as_secs_f32()
                );
                this.tree = DirNode::build(&g.file_list);
                let mut all = Vec::new();
                tree_rows(&this.tree, "", 0, None, &mut all);
                // small projects: everything open; big ones: first level only
                let limit = if g.file_list.len() <= 300 { usize::MAX } else { 0 };
                this.open_dirs = all.iter().filter(|r| r.dir && r.depth <= limit).map(|r| r.path.clone()).collect();
                this.file_syms.clear();
                for s in g.symbols.iter().filter(|s| s.parent.is_none()) {
                    *this.file_syms.entry(s.file.clone()).or_default() += 1;
                }
                if this.graph_root_is(&root_done) && g.files > 0 {
                    this.prefs.push_recent(root_done.clone());
                    this.prefs.save();
                }
                this.graph = g;
                this.loading = false;
                this.relayout();
                this.fit_pending = true;
                // files changed while this was loading
                if std::mem::take(&mut this.reload_pending) {
                    this.reload_graph(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Rebuild when a source file or tsconfig under the open folder changes. Dropping the old
    /// watcher closes its channel, which ends the task polling it.
    fn start_watching(&mut self, cx: &mut Context<Self>) {
        use std::sync::mpsc::TryRecvError;
        use std::time::Duration;
        self.watcher = None;
        let Some((w, rx)) = watch_folder(&self.root_path) else { return };
        self.watcher = Some(w);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(300)).await;
                let mut dirty = false;
                loop {
                    match rx.try_recv() {
                        Ok(()) => dirty = true,
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                if !dirty {
                    continue;
                }
                // let a burst (format on save, git checkout) settle before rebuilding once
                loop {
                    cx.background_executor().timer(Duration::from_millis(400)).await;
                    let mut more = false;
                    while rx.try_recv().is_ok() {
                        more = true;
                    }
                    if !more {
                        break;
                    }
                }
                if this.update(cx, |this, cx| this.reload_graph(cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Rebuild the graph of the open folder in place: selection, expanded cards, pins and the
    /// view stay where they are.
    fn reload_graph(&mut self, cx: &mut Context<Self>) {
        if self.root.is_empty() {
            return;
        }
        if self.loading || self.refreshing {
            self.reload_pending = true;
            return;
        }
        self.refreshing = true;
        let root = self.root_path.clone();
        let build_root = root.clone();
        let task = cx.background_spawn(async move {
            let t = Instant::now();
            let g = graph::build(&build_root);
            (g, t.elapsed())
        });
        cx.spawn(async move |this, cx| {
            let (g, took) = task.await;
            this.update(cx, |this, cx| {
                this.refreshing = false;
                if this.graph_root_is(&root) && !this.loading {
                    this.apply_reload(g, took);
                }
                if std::mem::take(&mut this.reload_pending) {
                    this.reload_graph(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Swap in a rebuilt graph, carrying symbol indexes over by (file, name, nth with that name).
    fn apply_reload(&mut self, g: Graph, took: std::time::Duration) {
        fn keys(g: &Graph) -> Vec<(String, String, usize)> {
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
        let old = keys(&self.graph);
        let fresh: HashMap<(String, String, usize), usize> =
            keys(&g).into_iter().enumerate().map(|(i, k)| (k, i)).collect();
        let carry = |i: usize| old.get(i).and_then(|k| fresh.get(k)).copied();

        self.selected = self.selected.and_then(carry);
        self.scope_seen = self.selected;
        self.filter_scope = self.filter_scope.and_then(carry);
        self.expanded = self.expanded.iter().filter_map(|&i| carry(i)).collect();
        self.pinned = self.pinned.iter().filter_map(|&i| carry(i)).collect();
        // anything holding indexes into the old graph
        self.cmp = None;
        self.flow = None;
        self.menu = None;
        self.hover_card = None;

        self.status = format!(
            "{} files · {} symbols · {} edges · reloaded in {:.1}s",
            g.files,
            g.symbols.len(),
            g.edges.len(),
            took.as_secs_f32()
        );
        if self.focus_file.as_ref().is_some_and(|f| !g.file_list.contains(f)) {
            self.focus_file = None;
        }
        self.tree = DirNode::build(&g.file_list);
        self.file_syms.clear();
        for s in g.symbols.iter().filter(|s| s.parent.is_none()) {
            *self.file_syms.entry(s.file.clone()).or_default() += 1;
        }
        self.graph = g;
        self.set_scope(self.filter_scope);
        // the code panel shows the file as it is now
        if let Some(f) = self.code.file.clone() {
            self.code.src = Some(source::load(&self.root_path.join(&f)).map(Rc::new));
            self.code.scrolled_for = None;
        }
        self.relayout();
    }

    /// Still showing `root` (another folder may have been opened while it loaded).
    fn graph_root_is(&self, root: &std::path::Path) -> bool {
        self.root_path == root
    }

    /// Lineage of `sel` among the drawn edges: (upstream, downstream), both including `sel`.
    /// Upstream = every visible node with a path to `sel`; downstream = everything `sel` leads to.
    fn lineage(&self, sel: usize) -> (HashSet<usize>, HashSet<usize>) {
        let walk = |forward: bool| {
            let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
            for e in &self.view.edges {
                let (a, b) = if forward { (e.from, e.to) } else { (e.to, e.from) };
                adj.entry(a).or_default().push(b);
            }
            let mut seen = HashSet::from([sel]);
            let mut q = VecDeque::from([sel]);
            while let Some(n) = q.pop_front() {
                for &m in adj.get(&n).map(Vec::as_slice).unwrap_or(&[]) {
                    if seen.insert(m) {
                        q.push_back(m);
                    }
                }
            }
            seen
        };
        (walk(false), walk(true))
    }

    /// Expanding `from` through `e` brings in its target. Every kind of link is drawn, but
    /// unless "Hooks & functions" is on, a component only opens into the components it renders.
    /// Hooks and functions always open into what they call and define (else they'd be dead ends).
    fn follows(&self, e: &Edge) -> bool {
        let g = &self.graph;
        e.from != e.to
            && e.kind != EdgeKind::Context
            && (self.show_functions
                || g.symbols[e.from].kind != SymKind::Component
                || g.symbols[e.to].kind == SymKind::Component)
    }

    /// (context nodes from other files that use the focused file, roots, isolated count)
    fn roots(&self) -> (Vec<usize>, Vec<usize>, usize) {
        let Some(f) = &self.focus_file else {
            let (r, iso) = self.entry_points();
            return (Vec::new(), r, iso);
        };
        let g = &self.graph;
        let mut roots: Vec<usize> = (0..g.symbols.len())
            .filter(|&i| g.symbols[i].parent.is_none() && &g.symbols[i].file == f)
            .collect();
        roots.sort_by_key(|&i| g.symbols[i].line);
        let mut seen = HashSet::new();
        let mut ctx = Vec::new();
        for &r in &roots {
            for e in g.in_edges(r) {
                if &g.symbols[e.from].file != f && seen.insert(e.from) {
                    ctx.push(e.from);
                }
            }
        }
        (ctx, roots, 0)
    }

    /// Warnings on symbols defined inside `i` that aren't drawn right now
    /// (a drawn child shows its own badge, so it isn't counted again here).
    fn hidden_warnings(&self, i: usize) -> usize {
        self.graph
            .out_edges(i)
            .filter(|e| e.kind == EdgeKind::Defines && !self.view.set.contains(&e.to))
            .map(|e| self.graph.symbols[e.to].warnings() + self.hidden_warnings(e.to))
            .sum()
    }

    /// Every symbol defined (at any depth) inside `i` that has warnings, in source order.
    fn warned_descendants(&self, i: usize, out: &mut Vec<usize>) {
        for e in self.graph.out_edges(i).filter(|e| e.kind == EdgeKind::Defines) {
            if self.graph.symbols[e.to].warnings() > 0 {
                out.push(e.to);
            }
            self.warned_descendants(e.to, out);
        }
    }

    /// Nothing references `i` except the symbol that defines it.
    fn is_unused(&self, i: usize) -> bool {
        !self.graph.in_edges(i).any(|e| e.from != i && !matches!(e.kind, EdgeKind::Defines | EdgeKind::Context))
    }

    /// Follow selection changes. The selection's own links are drawn (see `relayout`), so the
    /// canvas is rebuilt with the picked card kept in place. A new selection also becomes the
    /// filter scope, except one picked from the filter list (`scope_hold`); clearing it widens
    /// the scope back to the project.
    fn sync_scope(&mut self) {
        if self.selected == self.scope_seen {
            return;
        }
        let prev = self.scope_seen;
        self.scope_seen = self.selected;
        let hold = std::mem::take(&mut self.scope_hold);
        if !hold {
            self.set_scope(self.selected);
        }
        if self.filters.active() && !hold && self.selected.is_none() {
            self.refilter(true);
        } else {
            self.relayout_keep(self.selected.or(prev));
        }
    }

    /// Drop every scope: no picked file, no symbol scope (the selection stays if it's drawn).
    fn widen_to_project(&mut self) {
        self.set_scope(None);
        if self.focus_file.is_some() {
            self.open_file(None);
        } else {
            self.refilter(true);
        }
    }

    /// Widen a symbol scope back to the picked file (the trail's file crumb).
    fn widen_to_file(&mut self) {
        self.set_scope(None);
        self.selected = None;
        self.scope_seen = None;
        self.refilter(true);
    }

    /// Select `i` from the filter list: show it, but keep the filter scope as it is.
    fn pick_from_list(&mut self, i: usize) {
        let before = self.selected;
        self.reveal(i);
        if self.selected != before {
            self.scope_hold = true;
        }
    }

    fn set_scope(&mut self, s: Option<usize>) {
        self.filter_scope = s;
        self.scope_set.clear();
        let Some(s) = s else { return };
        // the selection and everything below it: what it renders, calls, defines and passes on
        let mut stack = vec![s];
        while let Some(n) = stack.pop() {
            if self.scope_set.insert(n) {
                stack.extend(self.graph.out_edges(n).map(|e| e.to));
            }
        }
    }

    /// Inside what's picked: the selected symbol and what's below it, else the picked file,
    /// else the whole project.
    fn in_scope(&self, i: usize) -> bool {
        match (self.filter_scope, &self.focus_file) {
            (Some(_), _) => self.scope_set.contains(&i),
            (None, Some(f)) => &self.graph.symbols[i].file == f,
            (None, None) => true,
        }
    }

    /// `i` passes every active sidebar filter and is inside the scope (true when none is active).
    fn filter_match(&self, i: usize) -> bool {
        self.in_scope(i) && self.filter_props(i)
    }

    /// `i` passes the filter chips, wherever it is.
    fn filter_props(&self, i: usize) -> bool {
        let f = &self.filters;
        let s = &self.graph.symbols[i];
        (!f.kinds.iter().any(|&k| k) || f.kinds[kind_index(s.kind)])
            && (!f.dup || !s.similar.is_empty())
            && (!f.exact || s.similar.iter().any(|m| m.exact))
            && (!f.warn || s.warnings() > 0)
            && (!f.memo || s.is_memoized())
            && (!f.unused || self.is_unused(i))
            && (!f.cycle || s.cycle.is_some())
    }

    /// What the canvas starts from while filtering: every match, and every card on a path to one
    /// from the scope (or, without one, from the usual roots of the project / focused file).
    /// Matches no path reaches stand on their own.
    fn filtered_nodes(&self, ctx: Vec<usize>, roots: Vec<usize>) -> Vec<usize> {
        let g = &self.graph;
        let n = g.symbols.len();
        let matches: Vec<usize> = (0..n)
            .filter(|&i| {
                self.filter_match(i)
                    && (self.filter_scope.is_some() || self.focus_file.as_ref().is_none_or(|f| &g.symbols[i].file == f))
            })
            .collect();
        let mut starts: Vec<usize> = match self.filter_scope {
            Some(s) => vec![s],
            None => ctx.into_iter().chain(roots).collect(),
        };
        // forward reach over every kind of link
        let mut reach = vec![false; n];
        let mut stack = starts.clone();
        let walk = |stack: &mut Vec<usize>, reach: &mut Vec<bool>| {
            while let Some(i) = stack.pop() {
                if !std::mem::replace(&mut reach[i], true) {
                    stack.extend(g.out_edges(i).map(|e| e.to));
                }
            }
        };
        walk(&mut stack, &mut reach);
        for &m in &matches {
            if !reach[m] {
                starts.push(m);
                stack.push(m);
                walk(&mut stack, &mut reach);
            }
        }
        // back from the matches, staying inside what the starts reach
        let mut keep = vec![false; n];
        let mut stack = matches.clone();
        while let Some(i) = stack.pop() {
            if reach[i] && !std::mem::replace(&mut keep[i], true) {
                stack.extend(g.in_edges(i).map(|e| e.from));
            }
        }
        if let Some(s) = self.filter_scope {
            keep[s] = true;
        }
        for &p in &self.pinned {
            keep[p] = true;
        }
        // breadth-first from the starts so the order follows the flow
        let mut out = Vec::new();
        let mut seen = vec![false; n];
        let mut queue: VecDeque<usize> = starts.into_iter().chain(self.pinned.iter().copied()).collect();
        while let Some(i) = queue.pop_front() {
            if !keep[i] || std::mem::replace(&mut seen[i], true) {
                continue;
            }
            out.push(i);
            queue.extend(g.out_edges(i).map(|e| e.to));
        }
        out
    }

    /// Rebuild the canvas after the filters or their scope changed.
    fn refilter(&mut self, refit: bool) {
        if refit {
            self.relayout();
            self.fit_pending = true;
        } else {
            self.relayout_keep(self.selected);
        }
    }

    /// `i` or a symbol defined inside it that isn't drawn right now passes the filters.
    fn filter_hit_within(&self, i: usize) -> bool {
        self.filter_match(i)
            || self
                .graph
                .out_edges(i)
                .filter(|e| e.kind == EdgeKind::Defines && !self.view.set.contains(&e.to))
                .any(|e| self.filter_hit_within(e.to))
    }

    /// Symbols passing the filters, grouped into clusters of code similar to each other
    /// (connected through `similar` links), biggest cluster first, each in source order.
    /// With a scope, clusters start from `hits` (in scope) but also pull in their twins outside it.
    fn dup_clusters(&self, hits: &[usize]) -> Vec<Vec<usize>> {
        let keep: HashSet<usize> = if self.filter_scope.is_some() {
            (0..self.graph.symbols.len()).filter(|&i| self.filter_props(i)).collect()
        } else {
            hits.iter().copied().collect()
        };
        let mut seen: HashSet<usize> = HashSet::new();
        let mut out = Vec::new();
        for &h in hits {
            if !seen.insert(h) {
                continue;
            }
            let mut group = vec![h];
            let mut stack = vec![h];
            while let Some(n) = stack.pop() {
                for m in &self.graph.symbols[n].similar {
                    if keep.contains(&m.other) && seen.insert(m.other) {
                        group.push(m.other);
                        stack.push(m.other);
                    }
                }
            }
            let g = &self.graph;
            group.sort_by(|&a, &b| (&g.symbols[a].file, g.symbols[a].line).cmp(&(&g.symbols[b].file, g.symbols[b].line)));
            out.push(group);
        }
        out.sort_by_key(|c| std::cmp::Reverse(c.len()));
        out
    }

    /// Best similar-code score of `i` or of a symbol defined inside it that isn't drawn right now.
    fn similar_score(&self, i: usize) -> Option<f32> {
        let own = self.graph.symbols[i].similar.first().map(|m| m.score);
        self.graph
            .out_edges(i)
            .filter(|e| e.kind == EdgeKind::Defines && !self.view.set.contains(&e.to))
            .filter_map(|e| self.similar_score(e.to))
            .chain(own)
            .max_by(f32::total_cmp)
    }

    /// `i` and every symbol defined inside it that has similar code elsewhere, in source order.
    fn similar_within(&self, i: usize, out: &mut Vec<usize>) {
        if !self.graph.symbols[i].similar.is_empty() {
            out.push(i);
        }
        for e in self.graph.out_edges(i).filter(|e| e.kind == EdgeKind::Defines) {
            self.similar_within(e.to, out);
        }
    }

    fn children(&self, i: usize) -> Vec<usize> {
        let mut seen = HashSet::new();
        self.graph
            .out_edges(i)
            .filter(|e| self.follows(e) && seen.insert(e.to))
            .map(|e| e.to)
            .collect()
    }

    /// Entry points: top-level components nobody renders (and that render something).
    fn entry_points(&self) -> (Vec<usize>, usize) {
        let g = &self.graph;
        let n = g.symbols.len();
        let (mut has_in, mut has_out) = (vec![false; n], vec![false; n]);
        for e in &g.edges {
            if self.follows(e) {
                has_in[e.to] = true;
                has_out[e.from] = true;
            }
        }
        let top = |i: usize| g.symbols[i].kind == SymKind::Component && g.symbols[i].parent.is_none();
        let mut roots: Vec<usize> = (0..n).filter(|&i| top(i) && !has_in[i] && has_out[i]).collect();
        let isolated = (0..n).filter(|&i| top(i) && !has_in[i] && !has_out[i]).count();
        if roots.is_empty() {
            // everything is in a cycle: fall back to every component that renders something
            roots = (0..n).filter(|&i| top(i) && has_out[i]).collect();
        }
        roots.sort_by(|&a, &b| {
            let (sa, sb) = (&g.symbols[a], &g.symbols[b]);
            (&sa.file, sa.line).cmp(&(&sb.file, sb.line))
        });
        (roots, isolated)
    }

    /// Recompute which nodes are shown (entry points + children of expanded nodes) and where.
    fn relayout(&mut self) {
        let (ctx, roots, isolated) = self.roots();
        self.entry_count = roots.len();
        self.isolated = isolated;
        let filtering = self.filters.active();
        let mut set: HashSet<usize> = HashSet::new();
        let mut order: Vec<usize> = Vec::new();
        let mut queue: VecDeque<usize> = if filtering {
            self.filtered_nodes(ctx, roots).into()
        } else {
            ctx.into_iter().chain(roots).chain(self.pinned.iter().copied()).collect()
        };
        while let Some(i) = queue.pop_front() {
            if !set.insert(i) {
                continue;
            }
            order.push(i);
            if self.expanded.contains(&i) {
                queue.extend(self.children(i));
            }
        }
        // who uses the selection (every kind of link), so picking a card shows where it's used.
        // What it uses / defines is left to expand and collapse: forcing those in too would make
        // collapsing the selected card do nothing.
        let sel = self.selected.filter(|s| set.contains(s));
        if let Some(s) = sel {
            for o in self.graph.in_edges(s).map(|e| e.from) {
                if o != s && set.insert(o) {
                    order.push(o);
                }
            }
        }
        let edges: Vec<Edge> = self
            .graph
            .edges
            .iter()
            // every kind of link between two drawn cards
            .filter(|e| e.from != e.to && set.contains(&e.from) && set.contains(&e.to))
            .copied()
            .collect();
        let (pos, size) = layout(&order, &edges, &self.graph);
        let has_in = edges.iter().map(|e| e.to).collect();
        let has_out = edges.iter().map(|e| e.from).collect();
        let kids = order.iter().map(|&i| (i, self.children(i).len())).collect();
        self.view = ViewData { nodes: order, set, edges, pos, size, has_in, has_out, kids };
    }

    fn screen_of(&self, i: usize) -> Option<(f32, f32)> {
        self.view
            .pos
            .get(&i)
            .map(|&(x, y)| (x * self.zoom + self.off.0, y * self.zoom + self.off.1))
    }

    /// The painted card (zoomed out, no text) under screen point (x, y); the last one drawn wins.
    fn flat_card_at(&self, x: f32, y: f32) -> Option<usize> {
        if self.zoom >= TEXT_ZOOM {
            return None;
        }
        let (w, h) = (NODE_W * self.zoom, NODE_H * self.zoom);
        self.view.nodes.iter().rev().copied().find(|&i| {
            self.screen_of(i).is_some_and(|(sx, sy)| x >= sx && x <= sx + w && y >= sy && y <= sy + h)
        })
    }

    /// A click on a card: select it (again: deselect); a double click also opens / closes it.
    fn click_card(&mut self, i: usize, clicks: usize) {
        if clicks >= 2 {
            self.toggle_expand(i);
            self.selected = Some(i);
        } else {
            self.selected = if self.selected == Some(i) { None } else { Some(i) };
        }
    }

    /// The right-click menu on a card: the detail panel's actions, without selecting first.
    fn card_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (i, (x, y)) = self.menu?;
        let s = self.graph.symbols.get(i)?;
        let kids = self.children(i).len();
        let open = self.expanded.contains(&i);
        let flowing = self.flow.as_ref().is_some_and(|f| f.trace.start == i);
        let file = s.file.clone();
        let other_file = self.focus_file.as_deref() != Some(file.as_str());

        let item = |id: &'static str, label: String| {
            div()
                .id(id)
                .px_3()
                .py(px(5.))
                .rounded(px(6.))
                .text_sm()
                .cursor_pointer()
                .hover(|st| st.bg(rgb(ACCENT_50)).text_color(rgb(ACCENT)))
                .child(label)
        };
        let sep = || div().my_1().h(px(1.)).bg(rgb(SOFT_BORDER));

        let mut m = card()
            .id("card-menu")
            .occlude()
            .w(px(220.))
            .p_1()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            // any press outside closes it (capture phase: a right-click on another card reopens it there)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .child(
                div()
                    .px_3()
                    .pt_1()
                    .pb(px(2.))
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(MUTED))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(s.name.clone()),
            )
            .child(
                item("menu-flow", if flowing { "■ Stop flow".into() } else { "▶ Play flow from here".into() })
                    .text_color(rgb(ACCENT))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        if flowing {
                            this.flow = None;
                        } else {
                            this.selected = Some(i);
                            this.start_flow();
                        }
                        cx.notify();
                    })),
            )
            .child(item("menu-focus", "Focus on item".into()).on_click(cx.listener(move |this, _, _, cx| {
                this.menu = None;
                this.reveal(i);
                cx.notify();
            })));
        if kids > 0 {
            m = m.child(
                item("menu-expand", if open { "Collapse".into() } else { format!("Expand ({kids})") }).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        this.toggle_expand(i);
                        cx.notify();
                    }),
                ),
            );
        }
        m = m
            .child(sep())
            .child(item("menu-source", "</> View source".into()).on_click(cx.listener(move |this, _, _, cx| {
                this.menu = None;
                this.selected = Some(i);
                this.source_tab = true;
                cx.notify();
            })))
            .child(item("menu-editor", "\u{2197} Open in editor".into()).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.menu = None;
                    this.open_in_editor(i, cx);
                    cx.notify();
                },
            )));
        if other_file {
            m = m.child(item("menu-file", format!("Show only {}", short_file(&file))).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.menu = None;
                    this.open_file(Some(file.clone()));
                    // narrow to the file, keep this symbol picked inside it
                    this.pick_from_list(i);
                    cx.notify();
                },
            )));
        }
        Some(
            deferred(anchored().position(point(px(x), px(y))).snap_to_window_with_margin(px(8.)).child(m))
                .with_priority(1)
                .into_any_element(),
        )
    }

    /// Relayout while keeping `anchor` at the same spot on screen (no jumping around).
    fn relayout_keep(&mut self, anchor: Option<usize>) {
        let before = anchor.and_then(|a| self.screen_of(a));
        self.relayout();
        if let (Some(a), Some((sx, sy))) = (anchor, before) {
            if let Some(&(x, y)) = self.view.pos.get(&a) {
                self.off = (sx - x * self.zoom, sy - y * self.zoom);
            }
        }
    }

    fn toggle_expand(&mut self, i: usize) {
        if !self.expanded.remove(&i) {
            self.expanded.insert(i);
        }
        self.relayout_keep(Some(i));
    }

    fn expand_all(&mut self) {
        let (_, roots, _) = self.roots();
        let mut seen: HashSet<usize> = HashSet::new();
        let mut stack: Vec<usize> = roots.into_iter().chain(self.pinned.iter().copied()).collect();
        while let Some(i) = stack.pop() {
            if seen.insert(i) {
                self.expanded.insert(i);
                stack.extend(self.children(i));
            }
        }
        self.relayout();
        self.fit_pending = true;
    }

    fn collapse_all(&mut self) {
        self.flow = None;
        self.expanded.clear();
        self.pinned.clear();
        self.relayout();
        self.fit_pending = true;
    }

    /// Expand the shortest path from something already visible down to `t`, select and center it.
    fn reveal(&mut self, t: usize) {
        if !self.view.set.contains(&t) {
            let mut next: HashMap<usize, usize> = HashMap::new(); // parent -> child toward t
            let mut queue = VecDeque::from([t]);
            let mut found = None;
            'bfs: while let Some(n) = queue.pop_front() {
                for e in self.graph.in_edges(n) {
                    if !self.follows(e) || e.from == t || next.contains_key(&e.from) {
                        continue;
                    }
                    next.insert(e.from, n);
                    if self.view.set.contains(&e.from) {
                        found = Some(e.from);
                        break 'bfs;
                    }
                    queue.push_back(e.from);
                }
            }
            match found {
                Some(mut n) => {
                    while n != t {
                        self.expanded.insert(n);
                        n = next[&n];
                    }
                }
                None => {
                    if !self.pinned.contains(&t) {
                        self.pinned.push(t);
                    }
                }
            }
            self.relayout();
            // filtered canvas: a path outside the filter isn't drawn, so show `t` on its own
            if !self.view.set.contains(&t) && !self.pinned.contains(&t) {
                self.pinned.push(t);
                self.relayout();
            }
        }
        self.selected = Some(t);
        self.center_on(t);
    }

    fn center_on(&mut self, i: usize) {
        let Some(&(x, y)) = self.view.pos.get(&i) else { return };
        let (vw, vh) = self.last_vp;
        let z = self.zoom.max(0.7);
        self.zoom = z;
        // center inside the free space between the left card and the detail panel
        let cx = (LEFT_W + vw - self.right_gap(vw)) / 2.0;
        self.off = (cx - (x + NODE_W / 2.0) * z, vh / 2.0 - (y + NODE_H / 2.0) * z);
    }

    /// Flattened indented flow: (depth, symbol, edge kind, repeated?)
    fn flow_list(&self, start: usize) -> Vec<(usize, usize, Option<EdgeKind>, bool)> {
        let mut out = Vec::new();
        let mut expanded: HashSet<usize> = HashSet::new();
        fn rec(
            v: &GraphView,
            n: usize,
            depth: usize,
            via: Option<EdgeKind>,
            expanded: &mut HashSet<usize>,
            out: &mut Vec<(usize, usize, Option<EdgeKind>, bool)>,
        ) {
            if out.len() >= 300 {
                return;
            }
            let again = !expanded.insert(n);
            out.push((depth, n, via, again));
            if again || depth >= 8 {
                return;
            }
            for e in v.graph.out_edges(n).filter(|e| e.kind != EdgeKind::Context) {
                rec(v, e.to, depth + 1, Some(e.kind), expanded, out);
            }
        }
        rec(self, start, 0, None, &mut expanded, &mut out);
        out
    }

    fn zoom_at(&mut self, factor: f32, cx: f32, cy: f32) {
        let nz = (self.zoom * factor).clamp(0.08, 2.5);
        let k = nz / self.zoom;
        self.off = (cx - (cx - self.off.0) * k, cy - (cy - self.off.1) * k);
        self.zoom = nz;
    }

    fn fit(&mut self, win_w: f32, win_h: f32) {
        let left = LEFT_W + MARGIN;
        let right = if self.showing_code() || self.cmp.is_some() { self.right_gap(win_w) } else { MARGIN };
        let aw = (win_w - left - right).max(100.0);
        let ah = (win_h - TOP_BAR).max(100.0);
        let (w, h) = self.view.size;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let z = ((aw - 80.0) / w).min((ah - 80.0) / h).clamp(0.08, 1.0);
        self.zoom = z;
        self.off = (left + (aw - w * z) / 2.0, TOP_BAR + (ah - h * z) / 2.0);
    }

    /// World rectangle covered by the minimap and its scale: (x0, y0, scale, pad_x, pad_y).
    fn minimap_xform(&self, vw: f32, vh: f32) -> (f32, f32, f32, f32, f32) {
        let z = self.zoom;
        let (vx0, vy0) = (-self.off.0 / z, -self.off.1 / z);
        let (vx1, vy1) = (vx0 + vw / z, vy0 + vh / z);
        let (w, h) = self.view.size;
        let (x0, y0) = (vx0.min(0.0), vy0.min(0.0));
        let (x1, y1) = (vx1.max(w), vy1.max(h));
        let inner = (MM_W - 16.0, MM_H - 16.0);
        let s = (inner.0 / (x1 - x0).max(1.0)).min(inner.1 / (y1 - y0).max(1.0));
        let pad = ((MM_W - (x1 - x0) * s) / 2.0, (MM_H - (y1 - y0) * s) / 2.0);
        (x0, y0, s, pad.0, pad.1)
    }

    fn minimap_jump(&mut self, sx: f32, sy: f32, vw: f32, vh: f32) {
        let (x0, y0, s, px_, py_) = self.minimap_xform(vw, vh);
        let (mx, my) = (vw - MARGIN - MM_W, vh - MARGIN - MM_H);
        let wx = x0 + (sx - mx - px_) / s;
        let wy = y0 + (sy - my - py_) / s;
        self.off = (vw / 2.0 - wx * self.zoom, vh / 2.0 - wy * self.zoom);
    }
}

// ---------------------------------------------------------------- ui bits

fn hsla(c: u32, a: f32) -> Hsla {
    let mut h: Hsla = rgb(c).into();
    h.a = a;
    h
}

type Pt = (f32, f32);

/// The curve every edge is drawn with (screen px), from the source handle to the target handle.
fn edge_curve(f: Pt, t: Pt, z: f32) -> [Pt; 4] {
    let k = ((t.0 - f.0).abs() / 2.0).max(60.0 * z);
    [f, (f.0 + k, f.1), (t.0 - k, t.1), t]
}

fn lerp(a: Pt, b: Pt, t: f32) -> Pt {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// The first part (0..t) of a cubic Bézier, as its own control points (de Casteljau);
/// its last point is the point at `t`.
fn bezier_head(c: [Pt; 4], t: f32) -> [Pt; 4] {
    let (p01, p12, p23) = (lerp(c[0], c[1], t), lerp(c[1], c[2], t), lerp(c[2], c[3], t));
    let (p012, p123) = (lerp(p01, p12, t), lerp(p12, p23, t));
    [c[0], p01, p012, lerp(p012, p123, t)]
}

/// The point at `t` on a cubic Bézier.
fn bezier_at(c: [Pt; 4], t: f32) -> Pt {
    let u = 1.0 - t;
    let (a, b, cc, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    (
        a * c[0].0 + b * c[1].0 + cc * c[2].0 + d * c[3].0,
        a * c[0].1 + b * c[1].1 + cc * c[2].1 + d * c[3].1,
    )
}

fn dist(a: Pt, b: Pt) -> f32 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

/// Edge strokes, tessellated by hand and merged into one path per colour.
///
/// `PathBuilder` runs lyon on every curve, every frame (dashes need a full arc-length
/// measurement too), and every overlapping path becomes its own GPU pass (clear + resolve
/// of a full-window MSAA texture). With hundreds of edges that made pan / zoom crawl.
/// Here each curve is flattened into a strip of quads and all curves of a colour share
/// one path, so a frame costs a few passes no matter how many edges are drawn.
#[derive(Default)]
struct StrokeBatch {
    /// in paint order: the first edge of each colour decides where that colour sits
    paths: Vec<(Hsla, gpui::Path<Pixels>)>,
}

impl StrokeBatch {
    /// Stroke the cubic Bézier `c` (px, relative to `origin`), solid or dashed (on, off).
    fn curve(&mut self, origin: Point<Pixels>, c: [Pt; 4], width: f32, dash: Option<(f32, f32)>, color: Hsla) {
        if dist(c[0], c[1]) + dist(c[1], c[2]) + dist(c[2], c[3]) < 0.5 {
            return;
        }
        // segments from how much the curve bends (Wang's formula), to within ~0.35px;
        // the curve is already in screen px, so this holds at any zoom
        let bend = |a: Pt, b: Pt, c: Pt| dist((0.0, 0.0), (a.0 - 2.0 * b.0 + c.0, a.1 - 2.0 * b.1 + c.1));
        let dd = bend(c[0], c[1], c[2]).max(bend(c[1], c[2], c[3]));
        let n = ((0.75 * dd / 0.35).sqrt().ceil() as usize).clamp(2, 64);
        let pts: Vec<Pt> = (0..=n).map(|i| bezier_at(c, i as f32 / n as f32)).collect();
        let mut acc = Vec::with_capacity(n + 1);
        acc.push(0.0f32);
        for w in pts.windows(2) {
            acc.push(acc.last().unwrap() + dist(w[0], w[1]));
        }
        let total = acc[n];
        let half = width / 2.0;
        // half-width offsets, along each point's normal
        let off: Vec<Pt> = (0..=n)
            .map(|i| {
                let (a, b) = (pts[i.saturating_sub(1)], pts[(i + 1).min(n)]);
                let (tx, ty) = (b.0 - a.0, b.1 - a.1);
                let l = (tx * tx + ty * ty).sqrt().max(1e-6);
                (-ty / l * half, tx / l * half)
            })
            .collect();
        // centre + offset at arc length `s`
        let sample = |s: f32| -> (Pt, Pt) {
            let j = acc.partition_point(|&a| a <= s).clamp(1, n) - 1;
            let t = ((s - acc[j]) / (acc[j + 1] - acc[j]).max(1e-6)).clamp(0.0, 1.0);
            (lerp(pts[j], pts[j + 1], t), lerp(off[j], off[j + 1], t))
        };
        let at = |p: Pt| origin + point(px(p.0), px(p.1));
        let i = match self.paths.iter().position(|(k, _)| *k == color) {
            Some(i) => i,
            None => {
                self.paths.push((color, gpui::Path::new(at(pts[0]))));
                self.paths.len() - 1
            }
        };
        let path = &mut self.paths[i].1;
        // solid (st = (0, 1) everywhere, like lyon's output) triangles for arc length s0..s1
        let st = (point(0.0, 1.0), point(0.0, 1.0), point(0.0, 1.0));
        let mut strip: Vec<(Pt, Pt)> = Vec::with_capacity(n + 2);
        let mut run = |s0: f32, s1: f32| {
            strip.clear();
            strip.push(sample(s0));
            for j in acc.partition_point(|&a| a <= s0)..n {
                if acc[j] >= s1 {
                    break;
                }
                strip.push((pts[j], off[j]));
            }
            strip.push(sample(s1));
            for w in strip.windows(2) {
                let ((p, a), (q, b)) = (w[0], w[1]);
                let (l0, r0) = (at((p.0 + a.0, p.1 + a.1)), at((p.0 - a.0, p.1 - a.1)));
                let (l1, r1) = (at((q.0 + b.0, q.1 + b.1)), at((q.0 - b.0, q.1 - b.1)));
                path.push_triangle((l0, r0, l1), st);
                path.push_triangle((r0, r1, l1), st);
            }
        };
        match dash {
            None => run(0.0, total),
            Some((on, gap)) => {
                let mut s = 0.0;
                while s < total {
                    run(s, (s + on).min(total));
                    s += on + gap;
                }
            }
        }
    }

    fn paint(self, window: &mut Window) {
        for (c, p) in self.paths {
            window.paint_path(p, c);
        }
    }
}

/// A card drawn zoomed out (no text): painted on the canvas instead of built as an element.
struct FlatCard {
    /// screen px: x, y, w, h
    bounds: (f32, f32, f32, f32),
    bg: Hsla,
    border: Hsla,
    border_w: f32,
    /// kind colour, along the top
    strip: Hsla,
    opacity: f32,
}

/// One arrow of the sequence diagram, ready to paint (px inside the diagram).
struct SeqArrow {
    /// x where it leaves / arrives
    a: f32,
    b: f32,
    y: f32,
    color: u32,
    alpha: f32,
    /// returns are dashed
    dashed: bool,
    /// drawn part, from `a` (0..1)
    q: f32,
    head: bool,
    /// moving dot: (colour, hollow)
    dot: Option<(u32, bool)>,
}

/// An activation bar on a lifeline: (x, y from, y to, top of the stack).
type SeqBar = (f32, f32, f32, bool);

/// A "Play flow" pulse on one drawn edge.
struct Pulse {
    from: Pt,
    to: Pt,
    /// lit part of the curve, from its start (0..1)
    trail: f32,
    /// moving dots: (position along the curve, is a return, runs on an event)
    heads: Vec<(f32, bool, bool)>,
}

fn kind_rgb(k: SymKind) -> u32 {
    match k {
        SymKind::Component => 0x3b82f6,
        SymKind::Hook => 0xa855f7,
        SymKind::Function => 0x10b981,
    }
}

fn kind_color(k: SymKind) -> Hsla {
    rgb(kind_rgb(k)).into()
}

/// `c` mixed into the card surface (white / dark grey): 0 = surface, 1 = `c` (opaque, so
/// nothing shows through a card).
fn tint(c: u32, k: f32) -> gpui::Rgba {
    let (base, c) = (pal(0xffffff), pal(c));
    let ch = |shift: u32| {
        let (a, b) = (((base >> shift) & 0xff) as f32 / 255.0, ((c >> shift) & 0xff) as f32 / 255.0);
        a + (b - a) * k
    };
    gpui::Rgba { r: ch(16), g: ch(8), b: ch(0), a: 1.0 }
}

/// Card fill while a flow plays: light once the flow has reached it, stronger while it runs.
fn flow_fill(k: SymKind, reached: bool, active: bool) -> Option<gpui::Rgba> {
    if active {
        Some(tint(kind_rgb(k), 0.34))
    } else if reached {
        Some(tint(kind_rgb(k), 0.13))
    } else {
        None
    }
}

fn edge_color(k: EdgeKind) -> Hsla {
    match k {
        EdgeKind::Renders => rgb(EDGE).into(),
        EdgeKind::Calls => rgb(0xc4b5fd).into(),
        EdgeKind::Handler => rgb(0x86efac).into(),
        EdgeKind::Defines => rgb(0xd1d5db).into(),
        EdgeKind::Context => rgb(0xf59e0b).into(),
    }
}

fn kind_name(k: SymKind) -> &'static str {
    match k {
        SymKind::Component => "component",
        SymKind::Hook => "hook",
        SymKind::Function => "function",
    }
}

/// (text, background) for a wrapper badge
fn wrap_colors(w: Wrap) -> (u32, u32) {
    match w {
        Wrap::ForwardRef => (0x475569, 0xf1f5f9),
        _ => (0x047857, 0xd1fae5),
    }
}

/// (text, background) for a dependency chip
fn origin_colors(o: Origin) -> (u32, u32) {
    match o {
        Origin::Prop | Origin::Param => (0x1d4ed8, 0xeff6ff),
        Origin::State => (0xb45309, 0xfef3c7),
        Origin::Context => (0x0f766e, 0xccfbf1),
        Origin::Memoized => (0x047857, 0xd1fae5),
        Origin::HookValue => (0x7e22ce, 0xf3e8ff),
        Origin::Unstable => (0xb91c1c, 0xfee2e2),
        Origin::Setter | Origin::Ref | Origin::Local | Origin::Outer => (0x475569, 0xf1f5f9),
    }
}

fn wrap_explainer(w: Wrap) -> &'static str {
    match w {
        Wrap::Memo => "React.memo: skips re-rendering when every prop is shallow-equal to last time.",
        Wrap::ForwardRef => "forwardRef: receives a ref from its parent as the second argument.",
        Wrap::UseCallback => "useCallback: keeps the same function identity until a dependency changes.",
        Wrap::UseMemo => "useMemo: recomputes the value only when a dependency changes.",
    }
}

/// Dependency chips + hints for one hook call.
fn dep_block(info: &DepInfo) -> Div {
    let mut b = div().flex().flex_col().gap_1();
    match &info.deps {
        None => {}
        Some(list) if list.is_empty() => {
            let once = if graph::is_effect(&info.hook) { "runs once, after the first render" } else { "created once" };
            b = b.child(div().text_xs().text_color(rgb(MUTED)).child(format!("[ ] — no dependencies, {once}")));
        }
        Some(list) => {
            b = b.child(div().flex().flex_row().flex_wrap().gap_1().children(list.iter().map(|d| {
                let (fg, bg) = origin_colors(d.origin);
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(6.))
                    .bg(rgb(bg))
                    .when(d.origin == Origin::Outer, |c| c.border_1().border_dashed().border_color(rgb(0xcbd5e1)))
                    .child(div().text_xs().font_family(MONO_FONT).text_color(rgb(INK)).child(d.text.clone()))
                    .child(div().text_size(px(10.)).text_color(rgb(fg)).child(d.origin_label()))
            })));
        }
    }
    let mut hints: Vec<_> = info.hints.iter().collect();
    hints.sort_by_key(|h| !h.warn);
    for h in hints {
        let (icon, fg) = if h.warn { ("⚠", 0xb45309) } else { ("ℹ", 0x64748b) };
        b = b.child(
            div()
                .flex()
                .flex_row()
                .gap_1()
                .child(div().text_xs().text_color(rgb(fg)).child(icon))
                .child(div().flex_1().min_w(px(0.)).text_xs().text_color(rgb(if h.warn { 0x92400e } else { MUTED })).child(h.text.clone())),
        );
    }
    b
}

/// One side of a compare row: line number and code, differing tokens tinted.
#[allow(clippy::too_many_arguments)]
fn cmp_half(
    src: &SourceFile,
    marks: &HashMap<usize, Vec<compare::Mark>>,
    pairs: &[compare::Pair],
    line: Option<usize>,
    changed: bool,
    gutter: f32,
    focus: Option<usize>,
    show_local: bool,
    hscroll: f32,
) -> Div {
    let cell = div().flex_1().min_w(px(0.)).h_full().overflow_hidden().flex().flex_row().items_center();
    let Some(l) = line else {
        // nothing on this side of the row
        return cell.bg(rgb(0xf3f4f6));
    };
    let tint: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> = marks
        .get(&l)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let bg = if focus == Some(m.pair) {
                ACCENT_200
            } else {
                match pairs[m.pair].kind {
                    DiffKind::Local if !show_local => return None,
                    DiffKind::Local => 0xe5e7eb,
                    DiffKind::Code => 0xfecaca,
                    DiffKind::Name | DiffKind::Literal => DUP_MARK,
                }
            };
            Some((m.range.clone(), gpui::HighlightStyle { background_color: Some(rgb(bg).into()), ..Default::default() }))
        })
        .collect();
    let text = if src.lines[l].is_empty() { " ".into() } else { src.lines[l].clone() };
    cell.when(changed, |d| d.bg(rgb(0xfff7e0)))
        .child(div().w(px(3.)).h_full().flex_shrink_0().when(changed, |d| d.bg(rgb(0xf59e0b))))
        .child(
            div()
                .w(px(gutter))
                .flex_shrink_0()
                .pr_2()
                .text_right()
                .text_color(rgb(0xa0a7b1))
                .child((l + 1).to_string()),
        )
        .child(
            div().flex_1().min_w(px(0.)).h_full().overflow_hidden().flex().items_center().child(
                div()
                    .relative()
                    .left(px(-hscroll))
                    .flex_shrink_0()
                    .child(StyledText::new(text).with_highlights(compare::overlay(&src.highlights[l], &tint))),
            ),
        )
}

/// `0.874` => "87%"
fn pct(score: f32) -> String {
    format!("{}%", (score * 100.0).floor() as u32)
}

fn card() -> Div {
    div()
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(BORDER))
        .rounded(px(12.))
        .shadow_md()
        .text_color(rgb(INK))
}

fn primary_button(id: &'static str, label: impl Into<String>) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py(px(6.))
        .rounded(px(8.))
        .bg(rgb(ACCENT))
        .text_color(rgb(ON_ACCENT))
        .text_sm()
        .font_weight(gpui::FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(|s| s.bg(rgb(ACCENT_DARK)))
        .child(label.into())
}

fn ghost_button(id: &'static str, label: impl Into<String>) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py(px(4.))
        .rounded(px(8.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(0xffffff))
        .text_color(rgb(INK))
        .text_xs()
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(ACCENT)).text_color(rgb(ACCENT)))
        .child(label.into())
}

fn section(t: &str) -> Div {
    div()
        .mt_4()
        .mb_1()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(rgb(MUTED))
        .child(t.to_uppercase())
}

/// Hover tooltip: a title line and an optional muted detail line.
struct Tip {
    title: gpui::SharedString,
    detail: Option<&'static str>,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(260.))
            .px_2()
            .py(px(5.))
            .rounded(px(6.))
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(BORDER))
            .shadow_md()
            .text_xs()
            .text_color(rgb(INK))
            .child(self.title.clone())
            .when_some(self.detail, |d, t| d.child(div().mt(px(2.)).text_color(rgb(MUTED)).child(t)))
    }
}

fn tip(title: impl Into<gpui::SharedString>, detail: Option<&'static str>) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    let title = title.into();
    move |_, cx| cx.new(|_| Tip { title: title.clone(), detail }).into()
}

/// Square icon-only action button (tool window header, search field).
fn tool_icon(id: &'static str, glyph: &'static str, size: f32, color: Option<u32>) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .w(px(24.))
        .h(px(24.))
        .rounded(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(MUTED))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(TITLE_HOVER)).text_color(rgb(INK)))
        .child(icon(glyph, size, color))
}

fn folder_icon() -> gpui::AnyElement {
    icon("\u{E8B7}", 13., Some(0x6c707e))
}

fn control_button(id: &'static str, label: &'static str, last: bool) -> Stateful<Div> {
    let b = div()
        .id(id)
        .w(px(26.))
        .h(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(0xfefefe))
        .text_color(rgb(INK))
        .text_sm()
        .cursor_pointer()
        .hover(|s| s.bg(rgb(0xf4f4f4)))
        .child(label);
    if last { b } else { b.border_b_1().border_color(rgb(0xeeeeee)) }
}

fn legend_dot(c: Hsla, t: &'static str) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(c))
        .child(div().text_xs().text_color(rgb(MUTED)).child(t))
}

/// "Overview | Source" switch in the detail panel.
fn tab_bar(source: bool, cx: &mut Context<GraphView>) -> Div {
    let tab = |id: &'static str, label: &'static str, active: bool| {
        div()
            .id(id)
            .px_3()
            .py(px(3.))
            .rounded(px(6.))
            .text_xs()
            .cursor_pointer()
            .when(active, |d| d.bg(rgb(0xffffff)).text_color(rgb(ACCENT)).font_weight(gpui::FontWeight::SEMIBOLD).shadow_sm())
            .when(!active, |d| d.text_color(rgb(MUTED)).hover(|s| s.text_color(rgb(INK))))
            .child(label)
    };
    div()
        .mt_2()
        .flex()
        .flex_row()
        .child(
            div()
                .flex()
                .flex_row()
                .p(px(2.))
                .gap(px(2.))
                .rounded(px(8.))
                .bg(rgb(0xf1f3f5))
                .child(tab("tab-overview", "Overview", !source).on_click(cx.listener(|this, _, _, cx| {
                    this.source_tab = false;
                    cx.notify();
                })))
                .child(tab("tab-source", "</> Source", source).on_click(cx.listener(|this, _, _, cx| {
                    this.source_tab = true;
                    this.code.scrolled_for = None;
                    cx.notify();
                }))),
        )
}

fn short_file(f: &str) -> &str {
    f.rsplit('/').next().unwrap_or(f)
}

/// The app logo (same drawing as `assets/app.ico`), with its edges in the current accent.
fn logo(sz: f32) -> impl IntoElement {
    let a = ACCENT;
    canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            let k = sz / 1024.0;
            let at = |x: f32, y: f32| b.origin + point(px(x * k), px(y * k));
            let rect = |x0: f32, y0: f32, x1: f32, y1: f32| Bounds::new(at(x0, y0), size(px((x1 - x0) * k), px((y1 - y0) * k)));
            window.paint_quad(fill(rect(0.0, 0.0, 1024.0, 1024.0), gpui::rgb(0x24223a)).corner_radii(px(200.0 * k)));
            // the small-size layout of the icon: big cards, thick edges, no text lines
            let (cw, ch, gap) = (330.0, 230.0, 250.0);
            let root = (70.0, 512.0 - ch / 2.0, 70.0 + cw, 512.0 + ch / 2.0, SymKind::Component);
            let kids = [
                (954.0 - cw, 512.0 - gap - ch / 2.0, 954.0, 512.0 - gap + ch / 2.0, SymKind::Hook),
                (954.0 - cw, 512.0 + gap - ch / 2.0, 954.0, 512.0 + gap + ch / 2.0, SymKind::Function),
            ];
            let src = (root.2, 512.0);
            let mut edges = StrokeBatch::default();
            for c in &kids {
                let dst = (c.0, (c.1 + c.3) / 2.0);
                let m = (dst.0 - src.0) * 0.6;
                let s = |p: Pt| (p.0 * k, p.1 * k);
                edges.curve(b.origin, [s(src), s((src.0 + m, src.1)), s((dst.0 - m, dst.1)), s(dst)], (64.0 * k).max(1.2), None, hsla(a, 1.0));
            }
            edges.paint(window);
            let r = px(30.0 * k);
            for (x0, y0, x1, y1, kind) in std::iter::once(root).chain(kids) {
                window.paint_quad(fill(rect(x0, y0, x1, y1), gpui::rgb(0xffffff)).corner_radii(r));
                window.paint_quad(fill(rect(x0, y0, x1, y0 + 56.0), gpui::rgb(kind_rgb(kind))).corner_radii(gpui::Corners {
                    top_left: r,
                    top_right: r,
                    bottom_right: px(0.0),
                    bottom_left: px(0.0),
                }));
            }
            let hr = 40.0 * k;
            for (x, y) in std::iter::once(src).chain(kids.iter().map(|c| (c.0, (c.1 + c.3) / 2.0))) {
                window.paint_quad(fill(rect(x - 40.0, y - 40.0, x + 40.0, y + 40.0), hsla(a, 1.0)).corner_radii(px(hr)));
            }
        },
    )
    .w(px(sz))
    .h(px(sz))
    .flex_shrink_0()
}

/// Project badge letters, JetBrains-style: first letters of the first two words
/// ("sample-react" → "SR"), or the first two letters of a single word.
fn initials(name: &str) -> String {
    let parts: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|p| !p.is_empty()).collect();
    let s: String = match parts.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect(),
        [a, b, ..] => a.chars().take(1).chain(b.chars().take(1)).collect(),
    };
    s.to_uppercase()
}

/// Rounded square with a project's initials, its colour picked from the name (stable per project).
fn badge(name: &str, d: f32) -> Div {
    const COLORS: [u32; 6] = [0xe5507a, 0x4f7cf0, 0x9a62e8, 0x2f9e78, 0xd9822b, 0x2a9fb5];
    let h = name.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    div()
        .flex_shrink_0()
        .w(px(d))
        .h(px(d))
        .rounded(px((d * 0.25).round()))
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgb(COLORS[h as usize % COLORS.len()]))
        .text_color(gpui::rgb(0xffffff))
        .text_size(px((d * 0.42).round()))
        .font_weight(gpui::FontWeight::BOLD)
        .child(initials(name))
}

fn folder_name(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| prefs::display(p))
}

impl GraphView {
    fn toggle_popup(&mut self, which: Popup, cx: &mut Context<Self>) {
        let just_closed = self.popup_closed.is_some_and(|(p, t)| p == which && t.elapsed().as_millis() < 300);
        self.popup = if self.popup == Some(which) || just_closed { None } else { Some(which) };
        self.popup_closed = None;
        cx.notify();
    }

    /// Recent projects as clickable rows (badge, name, path, ×). `ids` keeps the element ids
    /// apart where the list is shown twice.
    fn recent_list(&self, limit: usize, ids: (&'static str, &'static str), cx: &mut Context<Self>) -> Div {
        let mut list = div().flex().flex_col().gap(px(2.));
        for (i, path) in self.prefs.recent.iter().take(limit).enumerate() {
            let shown = prefs::display(path);
            let name = folder_name(path);
            let missing = !path.is_dir();
            let current = !self.root.is_empty() && self.root_path == *path;
            let (open_p, gone_p) = (path.clone(), path.clone());
            list = list.child(
                div()
                    .id((ids.0, i))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .px_2()
                    .py(px(5.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(current, |d| d.bg(rgb(ACCENT_50)))
                    .hover(|st| st.bg(rgb(HOVER)))
                    .when(missing, |d| d.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_recent(open_p.clone(), cx)))
                    .child(badge(&name, 28.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(INK))
                                    .truncate()
                                    .child(if missing { format!("{name} (missing)") } else { name }),
                            )
                            .child(div().text_xs().text_color(rgb(MUTED)).truncate().child(shown)),
                    )
                    .child(
                        div()
                            .id((ids.1, i))
                            .flex_shrink_0()
                            .w(px(22.))
                            .h(px(22.))
                            .rounded(px(5.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgb(0x9ca3af))
                            .hover(|st| st.bg(rgb(BORDER)).text_color(rgb(INK)))
                            .child(icon("\u{E711}", 10., None))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.forget_recent(&gone_p);
                                cx.notify();
                            })),
                    ),
            );
        }
        list
    }

    /// The project widget's drop-down (JetBrains "project widget"): open, then recent projects.
    fn project_popup(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let which = self.popup.filter(|p| *p == Popup::Project)?;
        let item = |id: &'static str, label: &'static str, hint: &'static str| {
            div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px_2()
                .py(px(5.))
                .rounded(px(6.))
                .text_sm()
                .text_color(rgb(INK))
                .cursor_pointer()
                .hover(|st| st.bg(rgb(HOVER)))
                .child(label)
                .child(div().text_xs().text_color(rgb(MUTED)).child(hint))
        };
        let sep = || div().my_1().mx_1().h(px(1.)).bg(rgb(SOFT_BORDER));
        let title = |t: &'static str| {
            div().px_2().pt_1().pb_1().text_xs().font_weight(gpui::FontWeight::SEMIBOLD).text_color(rgb(MUTED)).child(t)
        };
        let mut m = card()
            .id("project-popup")
            .occlude()
            .w(px(360.))
            .p_1()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                this.popup = None;
                this.popup_closed = Some((which, Instant::now()));
                cx.notify();
            }))
            .child(item("pp-open", "Open Folder…", "Ctrl+O").on_click(cx.listener(|this, _, _, cx| {
                this.popup = None;
                Self::prompt_open(cx);
                cx.notify();
            })))
            .child(sep())
            .child(title("Recent Projects"));
        if self.prefs.recent.is_empty() {
            m = m.child(div().px_2().pb_2().text_sm().text_color(rgb(MUTED)).child("Folders you open will show up here."));
        } else {
            m = m
                .child(self.recent_list(prefs::MAX_RECENT, ("recent-row", "recent-x"), cx))
                .child(sep())
                .child(item("pp-clear", "Clear Recent Projects", "").on_click(cx.listener(|this, _, _, cx| {
                    this.clear_recent();
                    cx.notify();
                })));
        }
        Some(
            deferred(anchored().position(point(px(40.), px(TB + 2.0))).snap_to_window_with_margin(px(8.)).child(m))
                .with_priority(2)
                .into_any_element(),
        )
    }

    /// The window's caption, JetBrains New UI-style: logo, project widget, then (right) the
    /// light/dark switch and the window buttons. Only the empty stretches drag the window.
    fn title_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.popup == Some(Popup::Project);
        let has = !self.root.is_empty();
        let name = if has { folder_name(&self.root_path) } else { "Open Project".to_string() };
        let widget = div()
            .id("project-widget")
            .flex_shrink_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .h(px(28.))
            .pl(px(if has { 6. } else { 10. }))
            .pr(px(8.))
            .rounded(px(6.))
            .cursor_pointer()
            .when(open, |d| d.bg(rgb(TITLE_HOVER)))
            .hover(|st| st.bg(rgb(TITLE_HOVER)))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_popup(Popup::Project, cx)))
            .when(has, |d| d.child(badge(&name, 20.0)))
            .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).text_color(rgb(INK)).child(name))
            .child(icon("\u{E70D}", 9., Some(MUTED)));
        let theme = div()
            .id("theme-toggle")
            .flex_shrink_0()
            .w(px(32.))
            .h(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|st| st.bg(rgb(TITLE_HOVER)))
            // shows the mode a click switches to
            .child(icon(if dark() { "\u{E706}" } else { "\u{E708}" }, 15., Some(INK)))
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_theme();
                cx.notify();
            }));
        let win_btn = |area: gpui::WindowControlArea, glyph: &'static str, close: bool| {
            div()
                .flex_shrink_0()
                .w(px(46.))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .font_family(ICON_FONT)
                .text_size(px(10.))
                .text_color(rgb(INK))
                .window_control_area(area)
                .hover(move |st| {
                    if close { st.bg(gpui::rgb(0xc42b1c)).text_color(gpui::rgb(0xffffff)) } else { st.bg(rgb(TITLE_HOVER)) }
                })
                .child(glyph)
        };
        let bar = div()
            .h(px(TB))
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_row()
            .items_center()
            .bg(rgb(TITLE_BG))
            .border_b_1()
            .border_color(rgb(BORDER))
            // macOS draws its own traffic lights at the top left (see `traffic_light_position`)
            .child(
                div()
                    .h_full()
                    .pl(px(if MAC { 80. } else { 12. }))
                    .pr(px(8.))
                    .flex()
                    .items_center()
                    .window_control_area(gpui::WindowControlArea::Drag)
                    .child(logo(20.0)),
            )
            .child(widget)
            .child(div().flex_1().h_full().window_control_area(gpui::WindowControlArea::Drag))
            .child(theme)
            .child(div().w(px(8.)).h_full().window_control_area(gpui::WindowControlArea::Drag));
        if MAC {
            return bar;
        }
        bar.child(win_btn(gpui::WindowControlArea::Min, "\u{E921}", false))
            .child(win_btn(gpui::WindowControlArea::Max, if window.is_maximized() { "\u{E923}" } else { "\u{E922}" }, false))
            .child(win_btn(gpui::WindowControlArea::Close, "\u{E8BB}", true))
    }

    /// Start page with no project open (JetBrains welcome screen): open button, recent projects.
    fn welcome(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let open = || primary_button("welcome-open", "Open Folder…").on_click(cx.listener(|_, _, _, cx| Self::prompt_open(cx)));
        let hint = |t: &'static str| div().text_xs().text_color(rgb(MUTED)).child(t);
        let body = if self.prefs.recent.is_empty() {
            card()
                .w(px(440.))
                .px_6()
                .py_6()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .child(logo(64.0))
                .child(div().mt_2().text_lg().font_weight(gpui::FontWeight::BOLD).child("Welcome to React Graph"))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .text_center()
                        .child("Open a project folder to see its component tree, starting from the entry points."),
                )
                .child(div().mt_3().child(open()))
                .child(hint("or press Ctrl+O"))
        } else {
            card()
                .w(px(620.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_5()
                        .py_4()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .child(logo(44.0))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .child(div().text_lg().font_weight(gpui::FontWeight::BOLD).child("React Graph"))
                                .child(hint(concat!("Version ", env!("CARGO_PKG_VERSION")))),
                        )
                        .child(open()),
                )
                .child(div().h(px(1.)).bg(rgb(SOFT_BORDER)))
                .child(
                    div()
                        .p_3()
                        .flex()
                        .flex_col()
                        .child(div().px_2().pb_2().text_xs().font_weight(gpui::FontWeight::SEMIBOLD).text_color(rgb(MUTED)).child("RECENT PROJECTS"))
                        .child(self.recent_list(prefs::MAX_RECENT, ("welcome-row", "welcome-x"), cx)),
                )
                .child(div().px_5().pb_3().child(hint("Ctrl+O opens a folder from anywhere.")))
        };
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .flex()
            .items_center()
            .justify_center()
            .child(body)
            .into_any_element()
    }
}

impl Render for GraphView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), (f32::from(vp.height) - TB));
        if self.last_vp == (0.0, 0.0) {
            self.fit_pending = true;
        }
        self.last_vp = (vw, vh);
        self.sync_scope();
        if self.fit_pending {
            self.fit(vw, vh);
            self.fit_pending = false;
        }
        if self.cmp.as_ref().is_some_and(|c| Some(c.from) != self.selected) {
            self.cmp = None;
        }
        if self.flow.as_ref().is_some_and(|f| Some(f.trace.start) != self.selected) {
            self.flow = None;
        }
        // "Play flow": advance the clock, and ask for another frame while it plays
        if let Some(f) = self.flow.as_mut().filter(|f| f.playing) {
            let now = Instant::now();
            if let Some(last) = f.last {
                f.t += (now - last).as_secs_f32().min(0.1) * self.flow_speed;
            }
            f.last = Some(now);
            if f.t >= f.trace.total {
                f.t = f.trace.total;
                f.playing = false;
                f.last = None;
            } else {
                window.request_animation_frame();
            }
        }
        let frame: Option<Frame> = self.flow.as_ref().map(|f| f.trace.frame(f.t));
        // sequence diagram instead of the graph (toolbar, minimap and zoom controls hide)
        let seq_on = self.seq_on();
        if seq_on {
            self.seq_scroll_update(vw, vh);
        }
        // the step playing now: its edge, and where in the code it happens
        let flow_step = self
            .flow
            .as_ref()
            .zip(frame.as_ref())
            .and_then(|(f, fr)| fr.current.map(|i| f.trace.steps[i].clone()));
        // keep the card the pulse is heading for on screen (unless the user is dragging)
        if let Some(s) = flow_step.as_ref().filter(|_| self.drag.is_none() && self.flow.as_ref().is_some_and(|f| f.playing)) {
            let target = if s.mv == Move::Go { s.edge.to } else { s.edge.from };
            if let Some((sx, sy)) = self.screen_of(target) {
                let (w, h) = (NODE_W * self.zoom, NODE_H * self.zoom);
                let (x0, x1) = (LEFT_W + MARGIN + 60.0, vw - self.right_gap(vw) - 40.0);
                let (y0, y1) = (TOP_BAR + 20.0, vh - 190.0);
                let dx = if sx < x0 { x0 - sx } else if sx + w > x1 { x1 - sx - w } else { 0.0 };
                let dy = if sy < y0 { y0 - sy } else if sy + h > y1 { y1 - sy - h } else { 0.0 };
                self.off = (self.off.0 + dx * 0.12, self.off.1 + dy * 0.12);
            }
        }
        // code panel: what to show (file, highlighted lines)
        let show_code = self.showing_code();
        let code_range = match self.selected {
            // while a flow plays: the line of the current call / render
            Some(_) if show_code && flow_step.is_some() => {
                let e = flow_step.as_ref().unwrap().edge;
                let file = self.graph.symbols[e.site].file.clone();
                self.ensure_code(&file, e.line);
                Some((e.line, e.line))
            }
            Some(s) if show_code => {
                let sym = &self.graph.symbols[s];
                let (file, range) = (sym.file.clone(), (sym.line, sym.end_line));
                self.ensure_code(&file, range.0);
                Some(range)
            }
            None if show_code => {
                let f = self.focus_file.clone().unwrap_or_default();
                self.ensure_code(&f, 1);
                None
            }
            _ => None,
        };
        let z = self.zoom;
        let (ox, oy) = self.off;
        let selected = self.selected;
        // a playing flow lights its own cards and edges; everything else fades
        let trace = self.flow.as_ref().map(|f| &f.trace);
        // focus mode: cards on the selected card's lineage stay lit, the rest fade
        let focus = selected
            .filter(|s| trace.is_none() && self.focus_selection && self.view.set.contains(s))
            .map(|s| self.lineage(s));
        let related = |i: usize| match trace {
            Some(t) => t.nodes.contains(&i),
            None => focus.as_ref().is_none_or(|(up, down)| up.contains(&i) || down.contains(&i)),
        };

        let to_screen = |x: f32, y: f32| (x * z + ox, y * z + oy);
        let on_screen = |x: f32, y: f32, w: f32, h: f32| {
            x + w >= -60.0 && y + h >= -60.0 && x <= vw + 60.0 && y <= vh + 60.0
        };

        // ---- background dots + edges (one canvas) ----
        // edge level: 0 = faded (off the focus lineage), 1 = normal, 2 = touches the selection
        let mut edge_list: Vec<((f32, f32), (f32, f32), EdgeKind, u8)> = Vec::new();
        // flow pulses: moving dots per edge, from the steps playing right now
        let mut heads: HashMap<flow::EdgeKey, Vec<(f32, bool, bool)>> = HashMap::new();
        if let (Some(t), Some(fr)) = (trace, frame.as_ref()) {
            for &(i, q) in &fr.moving {
                let s = &t.steps[i];
                let back = s.mv == Move::Back;
                heads.entry(flow::key(&s.edge)).or_default().push((if back { 1.0 - q } else { q }, back, s.later));
            }
        }
        let mut pulses: Vec<Pulse> = Vec::new();
        for e in &self.view.edges {
            let (Some(&a), Some(&b)) = (self.view.pos.get(&e.from), self.view.pos.get(&e.to)) else {
                continue;
            };
            let f = to_screen(a.0 + NODE_W, a.1 + NODE_H / 2.0);
            let t = to_screen(b.0, b.1 + NODE_H / 2.0);
            let (minx, maxx) = (f.0.min(t.0), f.0.max(t.0));
            let (miny, maxy) = (f.1.min(t.1), f.1.max(t.1));
            if on_screen(minx - 100.0, miny, maxx - minx + 200.0, maxy - miny) {
                let k = flow::key(e);
                let level = match trace {
                    Some(tr) => u8::from(tr.edges.contains(&k)),
                    None => {
                        let hot = selected == Some(e.from) || selected == Some(e.to);
                        let on_path = focus.as_ref().is_none_or(|(up, down)| {
                            (up.contains(&e.from) && up.contains(&e.to))
                                || (down.contains(&e.from) && down.contains(&e.to))
                        });
                        if hot { 2 } else if on_path { 1 } else { 0 }
                    }
                };
                edge_list.push((f, t, e.kind, level));
                if let Some(fr) = frame.as_ref() {
                    let trail = fr.trail.get(&k).copied().unwrap_or(0.0);
                    let hs = heads.remove(&k).unwrap_or_default();
                    if trail > 0.001 || !hs.is_empty() {
                        pulses.push(Pulse { from: f, to: t, trail, heads: hs });
                    }
                }
            }
        }
        // rings growing out of the cards a pulse just reached: (screen x, y, seconds since)
        let rings: Vec<(f32, f32, f32)> = frame
            .as_ref()
            .map(|fr| {
                fr.pings
                    .iter()
                    .filter(|&(_, &dt)| (0.0..PING).contains(&dt))
                    .filter_map(|(&n, &dt)| {
                        let &(x, y) = self.view.pos.get(&n)?;
                        let (sx, sy) = to_screen(x, y);
                        Some((sx, sy, dt))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // draw highlighted edges last so they sit on top
        edge_list.sort_by_key(|e| e.3);
        let has_graph = !self.view.nodes.is_empty();
        let bg_canvas = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let at = |p: Pt| bounds.origin + point(px(p.0), px(p.1));
                // dot grid (React Flow <Background variant="dots" />)
                let mut gap = 20.0 * z;
                while gap < 12.0 {
                    gap *= 2.0;
                }
                let dot = hsla(DOT, 0.55);
                let r = (1.4 * z.sqrt()).clamp(0.8, 1.6);
                let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                // thousands of dots: one layer, so the scene doesn't sort each one into its bounds tree
                window.paint_layer(bounds, |window| {
                    let mut y = oy.rem_euclid(gap);
                    while y < bh {
                        let mut x = ox.rem_euclid(gap);
                        while x < bw {
                            window.paint_quad(fill(
                                Bounds::new(bounds.origin + point(px(x - r / 2.0), px(y - r / 2.0)), size(px(r), px(r))),
                                dot,
                            ));
                            x += gap;
                        }
                        y += gap;
                    }
                });
                if !has_graph {
                    return;
                }
                let mut strokes = StrokeBatch::default();
                for ((fx, fy), (tx, ty), kind, level) in &edge_list {
                    let hot = *level == 2;
                    let c: Hsla = match level {
                        2 => rgb(ACCENT).into(),
                        1 => edge_color(*kind),
                        _ => Hsla { a: 0.25, ..edge_color(*kind) },
                    };
                    let c4 = edge_curve((*fx, *fy), (*tx, *ty), z);
                    let w = (if hot { 2.4 } else { 1.8 } * z.sqrt()).clamp(1.0, 3.0);
                    let dash = (*kind == EdgeKind::Context || (*kind == EdgeKind::Defines && z >= 0.35)).then_some((5.0, 4.0));
                    strokes.curve(bounds.origin, c4, w, dash, c);
                }

                // ---- "Play flow" ----
                // lit trails: the part of each edge a pulse has covered (and a return hasn't pulled back)
                for p in pulses.iter().filter(|p| p.trail > 0.001) {
                    let c4 = bezier_head(edge_curve(p.from, p.to, z), p.trail);
                    strokes.curve(bounds.origin, c4, (2.8 * z.sqrt()).clamp(1.6, 3.6), None, hsla(ACCENT, 0.9));
                }
                strokes.paint(window);
                // rings growing out of the cards a pulse just reached (cards are drawn on top)
                for &(sx, sy, dt) in &rings {
                    let k = dt / PING;
                    let grow = (2.0 + 16.0 * k) * z.sqrt();
                    let b = Bounds::new(
                        at((sx - grow, sy - grow)),
                        size(px(NODE_W * z + 2.0 * grow), px(NODE_H * z + 2.0 * grow)),
                    );
                    window.paint_quad(
                        gpui::outline(b, hsla(ACCENT, 0.85 * (1.0 - k)), gpui::BorderStyle::default())
                            .border_widths(px(2.0))
                            .corner_radii(px(12.0 * z + grow)),
                    );
                }
                // the moving dots: pink going in, teal coming back, hollow for event handlers
                for p in &pulses {
                    let curve = edge_curve(p.from, p.to, z);
                    for &(pos, back, hollow) in &p.heads {
                        let c = bezier_head(curve, pos)[3];
                        let r = (5.5 * z.sqrt()).clamp(3.5, 7.0);
                        let color = if back { TEAL } else { ACCENT };
                        let dot = |r: f32| Bounds::new(at((c.0 - r, c.1 - r)), size(px(2.0 * r), px(2.0 * r)));
                        window.paint_quad(fill(dot(r * 2.2), hsla(color, 0.18)).corner_radii(px(r * 2.2)));
                        window.paint_quad(fill(dot(r), hsla(color, 1.0)).corner_radii(px(r)));
                        if hollow {
                            window.paint_quad(fill(dot(r * 0.5), hsla(0xffffff, 1.0)).corner_radii(px(r * 0.5)));
                        }
                    }
                }
            },
        )
        .absolute()
        .size_full();

        // ---- canvas area (pan / zoom) ----
        let mut area = div()
            .id("area")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .overflow_hidden()
            .bg(rgb(CANVAS))
            .cursor(if self.drag.is_some() {
                gpui::CursorStyle::ClosedHand
            } else if self.hover_card.is_some() && z < TEXT_ZOOM {
                gpui::CursorStyle::PointingHand
            } else {
                gpui::CursorStyle::Arrow
            })
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                this.menu = None;
                let d = ev.delta.pixel_delta(px(20.));
                let factor = (1.0 + f32::from(d.y) * 0.002).clamp(0.6, 1.6);
                this.zoom_at(factor, f32::from(ev.position.x), f32::from(ev.position.y) - TB);
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, _| {
                    let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y) - TB);
                    this.drag = Some((x, y));
                    this.press = Some((x, y, ev.click_count));
                }),
            )
            // right-click on a painted card (zoomed out): its menu
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y) - TB);
                    this.menu = this.flat_card_at(x, y).map(|i| (i, (x, y + TB)));
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y) - TB);
                if let Some((lx, ly)) = this.drag {
                    if ev.dragging() {
                        this.off = (this.off.0 + x - lx, this.off.1 + y - ly);
                        this.drag = Some((x, y));
                    } else {
                        this.drag = None;
                    }
                    cx.notify();
                } else {
                    // painted cards have no hitbox of their own: track hover by hand
                    let hover = this.flat_card_at(x, y);
                    if hover != this.hover_card {
                        this.hover_card = hover;
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                    this.drag = None;
                    let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y) - TB);
                    // a press that didn't move: a click on the painted card under it, if any
                    if let Some((px_, py_, clicks)) = this.press.take()
                        && (x - px_).abs() + (y - py_).abs() < 4.0
                        && let Some(i) = this.flat_card_at(x, y)
                    {
                        this.click_card(i, clicks);
                    }
                    cx.notify();
                }),
            )
            .child(bg_canvas);

        let show_text = z >= TEXT_ZOOM;
        // shadows are a blur pass per element; skip them when nodes are tiny
        let fancy = z >= 0.6;
        let mut flat_cards: Vec<FlatCard> = Vec::new();
        let filtering = self.filters.active();
        for &i in &self.view.nodes {
            let Some(&(gx, gy)) = self.view.pos.get(&i) else { continue };
            let (sx, sy) = to_screen(gx, gy);
            let (nw, nh) = (NODE_W * z, NODE_H * z);
            if !on_screen(sx, sy, nw + 40.0, nh) {
                continue;
            }
            let s = &self.graph.symbols[i];
            let is_sel = selected == Some(i);
            // flow: reached cards get a pink border; the one running now (top of the stack,
            // or just reached in waves) is tinted too
            let (reached, active) = match (frame.as_ref(), trace) {
                (Some(fr), Some(t)) => (
                    fr.reached.contains(&i),
                    fr.stack.last() == Some(&i)
                        || (t.style == Style::Waves && fr.pings.get(&i).is_some_and(|&d| (0.0..PING).contains(&d))),
                ),
                _ => (false, false),
            };
            let color = kind_color(s.kind);
            // in file view, nodes from other files are context: draw them faded
            let dim = !is_sel && self.focus_file.as_ref().is_some_and(|f| &s.file != f);
            // focus mode: cards unrelated to the selection fade well back
            let faded = !related(i);
            // filtering: cards that are only on the way to a match stay readable but step back
            let filter_dim = filtering && !is_sel && !self.filter_hit_within(i);
            // a playing flow overrides file/filter dimming: every card on its path stays fully opaque
            let opacity = if faded {
                Some(0.2)
            } else if trace.is_none() && (dim || filter_dim) {
                Some(0.45)
            } else {
                None
            };
            // a playing flow fills its cards with their kind colour; otherwise selected = pink tint
            let bg = flow_fill(s.kind, reached, active)
                .unwrap_or_else(|| if is_sel { rgb(ACCENT_50) } else { rgb(0xffffff) });
            if !show_text {
                // zoomed out: just a box and its colour strip, painted (an element per card,
                // with its own layout, hitbox and listeners, was most of the frame on big graphs)
                let hover = self.hover_card == Some(i);
                flat_cards.push(FlatCard {
                    bounds: (sx, sy, nw, nh),
                    bg: bg.into(),
                    border: rgb(if is_sel || reached {
                        ACCENT
                    } else if hover {
                        0xcbd5e1
                    } else {
                        SOFT_BORDER
                    })
                    .into(),
                    border_w: if active { 2.0 } else { 1.0 },
                    strip: color,
                    opacity: opacity.unwrap_or(1.0),
                });
                continue;
            }
            let warns = s.warnings() + self.hidden_warnings(i);
            let dup = self.similar_score(i);
            let mut node = div()
                .when_some(opacity, |d, o| d.opacity(o))
                .id(("node", i))
                .absolute()
                .left(px(sx))
                .top(px(sy))
                .w(px(nw))
                .h(px(nh))
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(12.0 * z))
                .bg(bg)
                .border_1()
                .when(active, |d| d.border_2())
                .border_color(if is_sel || reached { rgb(ACCENT) } else { rgb(SOFT_BORDER) })
                .when(fancy, |d| d.shadow_sm())
                .cursor_pointer()
                .hover(|st| st.border_color(rgb(if is_sel || reached { ACCENT } else { 0xcbd5e1 })))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        this.menu = Some((i, (f32::from(ev.position.x), f32::from(ev.position.y))));
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                    this.click_card(i, ev.click_count());
                    cx.notify();
                }));
            node = node
                .child(
                    // header strip, like the React Flow hero nodes
                    div()
                        .h(px(22.0 * z))
                        .px(px(10.0 * z))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0 * z))
                        .border_b_1()
                        .border_color(rgb(SOFT_BORDER))
                        .child(div().flex_shrink_0().w(px(7.0 * z)).h(px(7.0 * z)).rounded_full().bg(color))
                        .child(
                            div()
                                .text_size(px(10.0 * z))
                                .font_family(MONO_FONT)
                                .text_color(rgb(MUTED))
                                .child(kind_name(s.kind)),
                        )
                        .children(s.wraps.iter().map(|w| {
                            let (fg, bg) = wrap_colors(*w);
                            div()
                                .px(px(4.0 * z))
                                .rounded(px(4.0 * z))
                                .bg(rgb(bg))
                                .text_size(px(9.0 * z))
                                .font_family(MONO_FONT)
                                .text_color(rgb(fg))
                                .child(w.label())
                        }))
                        .when(warns > 0 || dup.is_some(), |d| d.child(div().flex_1()))
                        .when_some(dup, |d, score| {
                            d.child(
                                div()
                                    .px(px(4.0 * z))
                                    .rounded(px(4.0 * z))
                                    .bg(rgb(DUP_BG))
                                    .text_size(px(9.5 * z))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(DUP_FG))
                                    .child(format!("⧉ {}", pct(score))),
                            )
                        })
                        .when(warns > 0, |d| {
                            d.child(
                                div()
                                    .px(px(4.0 * z))
                                    .rounded(px(4.0 * z))
                                    .bg(rgb(0xfef3c7))
                                    .text_size(px(9.5 * z))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(0xb45309))
                                    .child(format!("⚠ {warns}")),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .px(px(10.0 * z))
                        .flex()
                        .flex_col()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(13.5 * z))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(rgb(INK))
                                .whitespace_nowrap()
                                .child(s.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10.5 * z))
                                .text_color(rgb(MUTED))
                                .whitespace_nowrap()
                                .child(format!("{}:{}", short_file(&s.file), s.line)),
                        ),
                );
            area = area.child(node);

            // target handle (left)
            if show_text && self.view.has_in.contains(&i) {
                let hs = (10.0 * z).clamp(5.0, 10.0);
                area = area.child(
                    div()
                        .when(faded, |d| d.opacity(0.2))
                        .absolute()
                        .left(px(sx - hs / 2.0))
                        .top(px(sy + nh / 2.0 - hs / 2.0))
                        .w(px(hs))
                        .h(px(hs))
                        .rounded_full()
                        .bg(rgb(HANDLE))
                        .border_1()
                        .border_color(rgb(0xffffff)),
                );
            }

            // source handle (right) = expand / collapse toggle
            let kids = self.view.kids.get(&i).copied().unwrap_or(0);
            if kids > 0 && show_text {
                let open = self.expanded.contains(&i);
                let bh = (20.0 * z).clamp(14.0, 22.0);
                let label = if open { "−".to_string() } else { format!("+{kids}") };
                let bw = if open { bh } else { bh + 8.0 * (label.len() as f32 - 1.0).max(0.0) * (bh / 20.0) };
                area = area.child(
                    div()
                        .when(faded, |d| d.opacity(0.35))
                        .id(("exp", i))
                        .absolute()
                        .left(px(sx + nw - bh / 2.0))
                        .top(px(sy + nh / 2.0 - bh / 2.0))
                        .h(px(bh))
                        .min_w(px(bw))
                        .px(px(if open { 0.0 } else { 4.0 }))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(if open { rgb(0xffffff) } else { rgb(ACCENT) })
                        .border_1()
                        .border_color(if open { rgb(0xcbd5e1) } else { rgb(ACCENT) })
                        .text_color(if open { rgb(INK) } else { rgb(ON_ACCENT) })
                        .text_size(px((11.0 * z).clamp(9.0, 12.0)))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .shadow_sm()
                        .cursor_pointer()
                        .hover(|st| st.border_color(rgb(ACCENT)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_expand(i);
                            cx.notify();
                        }))
                        .child(label),
                );
            } else if show_text && self.view.has_out.contains(&i) {
                let hs = (10.0 * z).clamp(5.0, 10.0);
                area = area.child(
                    div()
                        .when(faded, |d| d.opacity(0.2))
                        .absolute()
                        .left(px(sx + nw - hs / 2.0))
                        .top(px(sy + nh / 2.0 - hs / 2.0))
                        .w(px(hs))
                        .h(px(hs))
                        .rounded_full()
                        .bg(rgb(HANDLE))
                        .border_1()
                        .border_color(rgb(0xffffff)),
                );
            }
        }
        if !flat_cards.is_empty() {
            area = area.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let r = 12.0 * z;
                        let strip_h = (5.0 * z).max(2.0);
                        for c in &flat_cards {
                            let fade = |mut h: Hsla| {
                                h.a *= c.opacity;
                                h
                            };
                            let (x, y, w, h) = c.bounds;
                            let b = Bounds::new(bounds.origin + point(px(x), px(y)), size(px(w), px(h)));
                            window.paint_quad(gpui::quad(
                                b,
                                px(r),
                                fade(c.bg),
                                px(c.border_w),
                                fade(c.border),
                                gpui::BorderStyle::default(),
                            ));
                            // the kind-coloured strip along the top, inside the border
                            let bw = c.border_w;
                            let inner = (r - bw).max(0.0);
                            let sb = Bounds::new(b.origin + point(px(bw), px(bw)), size(px(w - 2.0 * bw), px(strip_h)));
                            window.paint_quad(fill(sb, fade(c.strip)).corner_radii(gpui::Corners {
                                top_left: px(inner),
                                top_right: px(inner),
                                bottom_right: px(0.0),
                                bottom_left: px(0.0),
                            }));
                        }
                    },
                )
                .absolute()
                .size_full(),
            );
        }

        // ---- left sidebar: a JetBrains-style "Project" tool window ----
        let mut side = div()
            .id("sidebar")
            .occlude()
            .absolute()
            .top_0()
            .left_0()
            .bottom_0()
            .w(px(LEFT_W))
            .bg(rgb(0xf8f9fb))
            .border_r_1()
            .border_color(rgb(BORDER))
            .text_color(rgb(INK))
            .flex()
            .flex_col();
        let ready = !self.root.is_empty() && !self.loading;
        let filtering = self.filters.active();
        let has_q = !self.query.is_empty();
        // tool window header: title on the left, actions on the right
        let mut header = div()
            .h(px(36.))
            .flex_shrink_0()
            .pl_3()
            .pr(px(6.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .child(div().flex_1().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).child("Project"));
        if ready && filtering {
            header = header.child(
                tool_icon("tw-clear", "\u{E894}", 12., Some(ACCENT))
                    .tooltip(tip("Clear all filters", None))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filters = Filters::default();
                        this.refilter(true);
                        cx.notify();
                    })),
            );
        }
        if ready && !has_q && !filtering {
            header = header.child(
                tool_icon("tw-collapse", "\u{E73F}", 12., None).tooltip(tip("Collapse all folders", None)).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.open_dirs.clear();
                        cx.notify();
                    },
                )),
            );
        }
        // search field: magnifier, text (typing anywhere goes here), clear button
        let mut search = div()
            .id("search")
            .mx_3()
            .h(px(28.))
            .flex_shrink_0()
            .pl(px(8.))
            .pr(px(4.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .rounded(px(6.))
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(if has_q { ACCENT } else { 0xd3d5db }))
            .when(!has_q, |d| d.hover(|s| s.border_color(rgb(0xb4b8c1))))
            .child(icon("\u{E721}", 12., Some(if has_q { ACCENT } else { MUTED })))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_sm()
                    .text_color(rgb(if has_q { INK } else { 0x9ca3af }))
                    .child(if has_q { format!("{}▏", self.query) } else { "Search files, components, hooks…".to_string() }),
            );
        if has_q {
            search = search.child(
                tool_icon("search-clear", "\u{E711}", 9., None)
                    .w(px(20.))
                    .h(px(20.))
                    .tooltip(tip("Clear search", Some("Esc")))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.query.clear();
                        cx.notify();
                    })),
            );
        }
        side = side.child(header).child(search);

        // kind tabs (like Search Everywhere) + property filter toggles; counts are over the scope
        if ready {
            let g = &self.graph;
            let n = g.symbols.len();
            let count = |p: &dyn Fn(usize) -> bool| (0..n).filter(|&i| self.in_scope(i) && p(i)).count();
            let f = self.filters;
            let all = !f.kinds.iter().any(|&k| k);
            let kinds: [(&'static str, Option<SymKind>); 4] = [
                ("All", None),
                ("Components", Some(SymKind::Component)),
                ("Hooks", Some(SymKind::Hook)),
                ("Functions", Some(SymKind::Function)),
            ];
            let mut tabs = div().mt_2().mx_3().flex().flex_row().border_b_1().border_color(rgb(BORDER));
            for (t, (label, kind)) in kinds.into_iter().enumerate() {
                let ki = kind.map(kind_index);
                let on = ki.map_or(all, |k| f.kinds[k]);
                let num = match kind {
                    Some(k) => count(&|i| g.symbols[i].kind == k),
                    None => count(&|_| true),
                };
                let tip_title = format!("{label} · {num}");
                tabs = tabs.child(
                    div()
                        .id(("kind-tab", t))
                        .mb(px(-1.))
                        .px(px(7.))
                        .pt(px(4.))
                        .pb(px(5.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(5.))
                        .border_b_2()
                        .text_size(px(12.5))
                        .cursor_pointer()
                        .when(on, |d| d.border_color(rgb(ACCENT)).text_color(rgb(INK)))
                        .when(!on, |d| {
                            d.border_color(gpui::transparent_black()).text_color(rgb(MUTED)).hover(|s| s.text_color(rgb(INK)))
                        })
                        .when_some(kind, |d, k| d.child(div().w(px(6.)).h(px(6.)).flex_shrink_0().rounded_full().bg(kind_color(k))))
                        .child(label)
                        .tooltip(tip(tip_title, ki.map(|_| if MAC { "⌘-click to combine kinds" } else { "Ctrl+click to combine kinds" })))
                        .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                            let m = e.modifiers();
                            let ks = &mut this.filters.kinds;
                            match ki {
                                None => *ks = [false; 3],
                                Some(k) if m.control || m.platform || m.shift => ks[k] = !ks[k],
                                Some(k) => {
                                    let only = ks.iter().enumerate().all(|(j, &v)| v == (j == k));
                                    *ks = [false; 3];
                                    ks[k] = !only;
                                }
                            }
                            this.refilter(true);
                            cx.notify();
                        })),
                );
            }
            side = side.child(tabs);

            type Toggle = fn(&mut Filters);
            let props: [(&'static str, &'static str, &'static str, &'static str, usize, bool, u32, Toggle); 6] = [
                (
                    "f-dup",
                    "\u{E8C8}",
                    "Duplicates",
                    "Similar code exists elsewhere in the project",
                    count(&|i| !g.symbols[i].similar.is_empty()),
                    f.dup,
                    DUP_FG,
                    |f| f.dup = !f.dup,
                ),
                (
                    "f-exact",
                    "\u{E81E}",
                    "Identical",
                    "Has a twin with identical structure; only names or literals differ",
                    count(&|i| g.symbols[i].similar.iter().any(|m| m.exact)),
                    f.exact,
                    DUP_FG,
                    |f| f.exact = !f.exact,
                ),
                (
                    "f-warn",
                    "\u{E7BA}",
                    "Warnings",
                    "Dependency warnings on its own deps or the hooks it calls, or props that defeat a memo child",
                    count(&|i| g.symbols[i].warnings() > 0),
                    f.warn,
                    0xb45309,
                    |f| f.warn = !f.warn,
                ),
                (
                    "f-unused",
                    "\u{ED1A}",
                    "Unused",
                    "Nothing renders, calls or passes it",
                    count(&|i| self.is_unused(i)),
                    f.unused,
                    MUTED,
                    |f| f.unused = !f.unused,
                ),
                (
                    "f-memo",
                    "\u{E945}",
                    "Memoized",
                    "Wrapped in memo, useCallback or useMemo",
                    count(&|i| g.symbols[i].is_memoized()),
                    f.memo,
                    0x0e7490,
                    |f| f.memo = !f.memo,
                ),
                (
                    "f-cycle",
                    "\u{E8EE}",
                    "Cycles",
                    "Renders or calls itself through a loop (A \u{2192} B \u{2192} A)",
                    count(&|i| g.symbols[i].cycle.is_some()),
                    f.cycle,
                    0xbe185d,
                    |f| f.cycle = !f.cycle,
                ),
            ];
            let mut bar = div().mt(px(6.)).mx(px(9.)).flex().flex_row().items_center().gap(px(2.));
            for (id, glyph, name, detail, num, on, color, toggle) in props {
                let none = num == 0 && !on;
                bar = bar.child(
                    div()
                        .id(id)
                        .h(px(24.))
                        .px(px(6.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.))
                        .rounded(px(5.))
                        .border_1()
                        .cursor_pointer()
                        .when(on, |d| d.bg(tint(color, 0.14)).border_color(tint(color, 0.45)))
                        .when(!on, |d| d.border_color(gpui::transparent_black()).hover(|s| s.bg(rgb(TITLE_HOVER))))
                        .child(icon(glyph, 12., Some(if none { 0x9ca3af } else { color })))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(if on {
                                    color
                                } else if none {
                                    0x9ca3af
                                } else {
                                    INK
                                }))
                                .child(num.to_string()),
                        )
                        .tooltip(tip(format!("{name} · {num}{}", if on { " (on)" } else { "" }), Some(detail)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            toggle(&mut this.filters);
                            this.refilter(true);
                            cx.notify();
                        })),
                );
            }
            side = side.child(bar);

            // scope: the counts only look at the selection and what's below it
            let scope_label = match (self.filter_scope, &self.focus_file) {
                (Some(sc), _) => Some((Some(g.symbols[sc].kind), g.symbols[sc].name.clone(), self.scope_set.len())),
                (None, Some(f)) => Some((None, short_file(f).to_string(), g.symbols.iter().filter(|s| &s.file == f).count())),
                (None, None) => None,
            };
            if let Some((kind, name, size)) = scope_label {
                side = side.child(
                    div()
                        .mx_3()
                        .mt(px(6.))
                        .px_2()
                        .h(px(24.))
                        .flex_shrink_0()
                        .rounded(px(5.))
                        .bg(rgb(ACCENT_50))
                        .border_1()
                        .border_color(rgb(ACCENT_200))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.))
                        .text_xs()
                        .child(match kind {
                            Some(k) => div().w(px(7.)).h(px(7.)).flex_shrink_0().rounded_full().bg(kind_color(k)).into_any_element(),
                            None => icon("\u{E8A5}", 11., Some(MUTED)),
                        })
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(format!("In {name} · {size} symbol{}", if size == 1 { "" } else { "s" })),
                        )
                        .child(
                            tool_icon("f-unscope", "\u{E711}", 8., None)
                                .w(px(18.))
                                .h(px(18.))
                                .tooltip(tip("Search the whole project", None))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.widen_to_project();
                                    cx.notify();
                                })),
                        ),
                );
            }
        }

        let row = |id: (&'static str, usize), depth: usize, active: bool| {
            div()
                .id(id)
                .h(px(24.))
                .flex_shrink_0()
                .mx(px(6.))
                .rounded(px(5.))
                .pl(px(6. + depth as f32 * 16.))
                .pr(px(6.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .cursor_pointer()
                .when(active, |d| d.bg(rgb(ACCENT_100)).text_color(rgb(ACCENT)))
                .when(!active, |d| d.hover(|s| s.bg(rgb(TITLE_HOVER))))
        };
        let file_icon = |path: &str| {
            let (label, color) = file_badge(path);
            div()
                .w(px(24.))
                .flex_shrink_0()
                .text_size(px(9.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(color))
                .child(label)
        };
        let name_cell = |t: String| div().flex_1().overflow_hidden().whitespace_nowrap().text_sm().child(t);

        let mut list: Vec<gpui::AnyElement> = Vec::new();
        let sel_file = selected.map(|s| self.graph.symbols[s].file.clone());
        if !self.query.is_empty() {
            let q = self.query.to_lowercase();
            // files first
            let filtering = self.filters.active();
            let hit_files: HashSet<&str> = if filtering {
                (0..self.graph.symbols.len())
                    .filter(|&i| self.filter_match(i))
                    .map(|i| self.graph.symbols[i].file.as_str())
                    .collect()
            } else {
                HashSet::new()
            };
            let files: Vec<&String> = self
                .graph
                .file_list
                .iter()
                .filter(|f| f.to_lowercase().contains(&q) && (!filtering || hit_files.contains(f.as_str())))
                .take(8)
                .collect();
            if !files.is_empty() {
                list.push(section("Files").px_3().into_any_element());
            }
            for (n, f) in files.into_iter().enumerate() {
                let path = f.clone();
                let dir = f.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
                list.push(
                    row(("fhit", n), 0, false)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.query.clear();
                            this.open_file(Some(path.clone()));
                            cx.notify();
                        }))
                        .child(file_icon(f))
                        .child(name_cell(short_file(f).to_string()))
                        .child(div().text_xs().text_color(rgb(MUTED)).whitespace_nowrap().child(dir))
                        .into_any_element(),
                );
            }
            let mut hits: Vec<usize> = (0..self.graph.symbols.len())
                .filter(|&i| self.graph.symbols[i].name.to_lowercase().contains(&q) && self.filter_match(i))
                .collect();
            hits.sort_by_key(|&i| {
                let s = &self.graph.symbols[i];
                (!s.name.to_lowercase().starts_with(&q), s.name.len())
            });
            let total = hits.len();
            if total > 0 {
                list.push(section("Symbols").px_3().into_any_element());
            }
            for (n, i) in hits.into_iter().take(30).enumerate() {
                let t = &self.graph.symbols[i];
                list.push(
                    row(("hit", n), 0, false)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.query.clear();
                            if let Some(f) = this.focus_file.clone() {
                                let file = this.graph.symbols[i].file.clone();
                                if file != f {
                                    this.open_file(Some(file));
                                }
                            }
                            this.reveal(i);
                            cx.notify();
                        }))
                        .child(div().w(px(7.)).h(px(7.)).flex_shrink_0().rounded_full().bg(kind_color(t.kind)))
                        .child(name_cell(t.name.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .whitespace_nowrap()
                                .child(format!("{}:{}", short_file(&t.file), t.line)),
                        )
                        .into_any_element(),
                );
            }
            if total > 30 {
                list.push(
                    div()
                        .px_3()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(format!("…and {} more", total - 30))
                        .into_any_element(),
                );
            }
            if list.is_empty() {
                list.push(div().px_3().mt_2().text_xs().text_color(rgb(MUTED)).child("No matches").into_any_element());
            }
        } else if self.filters.active() && !self.root.is_empty() && !self.loading {
            const MAX: usize = 300;
            let g = &self.graph;
            let mut hits: Vec<usize> = (0..g.symbols.len()).filter(|&i| self.filter_match(i)).collect();
            hits.sort_by(|&a, &b| (&g.symbols[a].file, g.symbols[a].line).cmp(&(&g.symbols[b].file, g.symbols[b].line)));
            let total = hits.len();
            let dup_mode = self.filters.dup || self.filters.exact;
            let clustered = dup_mode && self.filter_by_cluster;
            let mut head = div()
                .px_3()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(section(&format!("{total} match{}", if total == 1 { "" } else { "es" })));
            if dup_mode {
                let by_cluster = self.filter_by_cluster;
                head = head.child(
                    div()
                        .id("f-group")
                        .mt_4()
                        .mb_1()
                        .text_xs()
                        .text_color(rgb(ACCENT))
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .child(if by_cluster { "Group by file" } else { "Group by similarity" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.filter_by_cluster = !by_cluster;
                            cx.notify();
                        })),
                );
            }
            list.push(head.into_any_element());
            // (header label, best pair to compare, members)
            let groups: Vec<(String, Option<(usize, usize)>, Vec<usize>)> = if clustered {
                self.dup_clusters(&hits)
                    .into_iter()
                    .map(|c| {
                        let best = c
                            .iter()
                            .flat_map(|&i| g.symbols[i].similar.iter().filter(|m| c.contains(&m.other)).map(move |m| (i, *m)))
                            .max_by(|x, y| x.1.score.partial_cmp(&y.1.score).unwrap_or(std::cmp::Ordering::Equal));
                        let label = match best {
                            Some((_, m)) if m.exact => format!("{} × identical structure", c.len()),
                            Some((_, m)) => format!("{} × up to {} similar", c.len(), pct(m.score)),
                            None => format!("{} × (twin filtered out)", c.len()),
                        };
                        (label, best.map(|(i, m)| (i, m.other)), c)
                    })
                    .collect()
            } else {
                let mut out: Vec<(String, Option<(usize, usize)>, Vec<usize>)> = Vec::new();
                for i in hits {
                    let f = &g.symbols[i].file;
                    match out.last_mut() {
                        Some(last) if &g.symbols[last.2[0]].file == f => last.2.push(i),
                        _ => out.push((f.clone(), None, vec![i])),
                    }
                }
                out
            };
            let mut shown = 0usize;
            for (gi, (label, pair, members)) in groups.into_iter().enumerate() {
                if shown >= MAX {
                    break;
                }
                let header = if clustered {
                    let mut h = div()
                        .id(("fgroup", gi))
                        .mt_2()
                        .px_3()
                        .h(px(22.))
                        .flex_shrink_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_xs()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(rgb(DUP_FG))
                                .child(label),
                        );
                    if let Some((a, b)) = pair {
                        h = h.child(
                            div()
                                .id(("fcmp", gi))
                                .px(px(6.))
                                .rounded(px(4.))
                                .bg(rgb(DUP_BG))
                                .text_xs()
                                .text_color(rgb(DUP_FG))
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(DUP_MARK)))
                                .child("Compare")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.pick_from_list(a);
                                    this.open_compare(a, b);
                                    cx.notify();
                                })),
                        );
                    }
                    h.into_any_element()
                } else {
                    let path = label;
                    let dir = path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
                    row(("fgroup", gi), 0, self.focus_file.as_deref() == Some(path.as_str()))
                        .mt_1()
                        .child(file_icon(&path))
                        .child(name_cell(short_file(&path).to_string()))
                        .child(div().text_xs().text_color(rgb(MUTED)).whitespace_nowrap().child(dir))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_file(Some(path.clone()));
                            cx.notify();
                        }))
                        .into_any_element()
                };
                list.push(header);
                for i in members {
                    if shown >= MAX {
                        break;
                    }
                    let t = &g.symbols[i];
                    let warns = t.warnings();
                    let dup = t.similar.first().map(|m| m.score);
                    list.push(
                        row(("fhit", shown), 1, selected == Some(i))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(f) = this.focus_file.clone() {
                                    let file = this.graph.symbols[i].file.clone();
                                    if file != f {
                                        this.open_file(Some(file));
                                    }
                                }
                                this.pick_from_list(i);
                                cx.notify();
                            }))
                            .child(div().w(px(7.)).h(px(7.)).flex_shrink_0().rounded_full().bg(kind_color(t.kind)))
                            .child(name_cell(t.name.clone()))
                            .when(!self.in_scope(i), |d| {
                                d.text_color(rgb(MUTED))
                                    .child(div().text_xs().text_color(rgb(0x9ca3af)).whitespace_nowrap().child("outside"))
                            })
                            .when_some(dup.filter(|_| dup_mode && !clustered), |d, score| {
                                d.child(div().text_xs().text_color(rgb(DUP_FG)).whitespace_nowrap().child(pct(score)))
                            })
                            .when(warns > 0, |d| {
                                d.child(
                                    div().text_xs().text_color(rgb(0xb45309)).whitespace_nowrap().child(format!("⚠ {warns}")),
                                )
                            })
                            .child(div().text_xs().text_color(rgb(MUTED)).whitespace_nowrap().child(if clustered {
                                format!("{}:{}", short_file(&t.file), t.line)
                            } else {
                                format!(":{}", t.line)
                            }))
                            .into_any_element(),
                    );
                    shown += 1;
                }
            }
            if total > shown {
                list.push(
                    div()
                        .px_3()
                        .mt_1()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(format!("…and {} more", total - shown))
                        .into_any_element(),
                );
            }
            if total == 0 {
                list.push(
                    div()
                        .px_3()
                        .mt_2()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("Nothing matches these filters")
                        .into_any_element(),
                );
            }
        } else if !self.root.is_empty() && !self.loading {
            let chevron = |open: bool| {
                div()
                    .w(px(12.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .child(icon(if open { "\u{E70D}" } else { "\u{E76C}" }, 8., Some(MUTED)))
            };
            let project = self
                .root
                .trim_end_matches(['\\', '/'])
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or("")
                .to_string();
            list.push(
                row(("proj", 0), 0, self.focus_file.is_none())
                    .mt_1()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_file(None);
                        cx.notify();
                    }))
                    .child(chevron(true))
                    .child(folder_icon())
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(project),
                    )
                    .child(div().text_xs().text_color(rgb(MUTED)).whitespace_nowrap().child("whole project"))
                    .into_any_element(),
            );
            let mut rows = Vec::new();
            tree_rows(&self.tree, "", 0, Some(&self.open_dirs), &mut rows);
            for (n, r) in rows.into_iter().enumerate() {
                let path = r.path.clone();
                let depth = r.depth + 1;
                if r.dir {
                    list.push(
                        row(("dir", n), depth, false)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.open_dirs.remove(&path) {
                                    this.open_dirs.insert(path.clone());
                                }
                                cx.notify();
                            }))
                            .child(chevron(r.open))
                            .child(folder_icon())
                            .child(name_cell(r.name))
                            .into_any_element(),
                    );
                } else {
                    let active = self.focus_file.as_deref() == Some(r.path.as_str());
                    let count = self.file_syms.get(&r.path).copied().unwrap_or(0);
                    let holds_sel = !active && sel_file.as_deref() == Some(r.path.as_str());
                    list.push(
                        row(("file", n), depth, active)
                            .when(holds_sel, |d| d.bg(rgb(0xeef0f3)))
                            .when(count == 0 && !active, |d| d.text_color(rgb(0x9ca3af)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_file(Some(path.clone()));
                                cx.notify();
                            }))
                            .child(div().w(px(12.)).flex_shrink_0())
                            .child(file_icon(&r.path))
                            .child(name_cell(r.name))
                            .when(count > 0, |d| {
                                d.child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(if active { ACCENT } else { 0x9ca3af }))
                                        .child(count.to_string()),
                                )
                            })
                            .into_any_element(),
                    );
                }
            }
        }
        side = side.child(
            div()
                .id("tree")
                .mt_2()
                .border_t_1()
                .border_color(rgb(BORDER))
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .pb_3()
                .flex()
                .flex_col()
                .children(list),
        );
        // status bar strip at the bottom of the tool window
        if !self.status.is_empty() {
            side = side.child(
                div()
                    .h(px(26.))
                    .flex_shrink_0()
                    .px_3()
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(self.status.clone()),
            );
        }

        // ---- toolbar above the canvas: what is shown + view options ----
        let toolbar = (!self.root.is_empty() && !self.loading && !seq_on).then(|| {
            let right_gap = self.right_gap(vw);
            // trail of what's picked: Whole project › file › symbol; earlier crumbs widen back out
            let mut crumbs = div().flex().flex_row().flex_wrap().items_center().gap_1().text_sm();
            let sep = || div().text_color(rgb(0xcbd5e1)).child("›");
            let crumb = |id: &'static str, label: String, current: bool| {
                div()
                    .id(id)
                    .when(current, |d| d.font_weight(gpui::FontWeight::SEMIBOLD).text_color(rgb(INK)))
                    .when(!current, |d| d.text_color(rgb(ACCENT)).cursor_pointer().hover(|s| s.underline()))
                    .child(label)
            };
            let at_top = self.focus_file.is_none() && selected.is_none();
            crumbs = crumbs.child(crumb("crumb-project", "Whole project".into(), at_top).when(!at_top, |d| {
                d.on_click(cx.listener(|this, _, _, cx| {
                    this.selected = None;
                    this.widen_to_project();
                    cx.notify();
                }))
            }));
            if at_top {
                crumbs = crumbs.child(div().text_color(rgb(MUTED)).child("· from entry points"));
            }
            if let Some(f) = &self.focus_file {
                let (dir, name) = f.rsplit_once('/').unwrap_or(("", f.as_str()));
                crumbs = crumbs.child(sep());
                if !dir.is_empty() {
                    crumbs = crumbs.child(div().text_color(rgb(MUTED)).child(format!("{dir}/")));
                }
                let current = selected.is_none();
                crumbs = crumbs.child(crumb("crumb-file", name.to_string(), current).when(!current, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| {
                        this.widen_to_file();
                        cx.notify();
                    }))
                }));
            }
            if let Some(s) = selected {
                let t = &self.graph.symbols[s];
                crumbs = crumbs
                    .child(sep())
                    .child(div().w(px(7.)).h(px(7.)).rounded_full().bg(kind_color(t.kind)))
                    .child(crumb("crumb-symbol", t.name.clone(), true));
            }
            let mut buttons = div().flex().flex_row().flex_wrap().gap_1();
            if self.focus_file.is_some() {
                let on = self.file_panel && selected.is_none();
                buttons = buttons.child(
                    ghost_button("view-file-source", if on { "✓ Source" } else { "</> Source" }).on_click(
                        cx.listener(move |this, _, _, cx| {
                            if on {
                                this.file_panel = false;
                            } else {
                                this.file_panel = true;
                                this.selected = None;
                                this.code.scrolled_for = None;
                            }
                            cx.notify();
                        }),
                    ),
                );
            }
            buttons = buttons
                .child(ghost_button("expand-all", "Expand all").on_click(cx.listener(|this, _, _, cx| {
                    this.expand_all();
                    cx.notify();
                })))
                .child(ghost_button("collapse-all", "Collapse all").on_click(cx.listener(|this, _, _, cx| {
                    this.collapse_all();
                    cx.notify();
                })))
                .child(
                    ghost_button(
                        "focus-toggle",
                        if self.focus_selection { "✓ Dim others" } else { "Dim others" },
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.focus_selection = !this.focus_selection;
                        cx.notify();
                    })),
                );
            // which cards a component opens into (every kind of link is always drawn)
            buttons = buttons.child(
                ghost_button(
                    "fn-toggle",
                    if self.show_functions { "✓ Hooks & functions" } else { "Hooks & functions" },
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_functions = !this.show_functions;
                    // cards a playing trace runs through may disappear
                    this.flow = None;
                    this.relayout();
                    this.fit_pending = true;
                    cx.notify();
                })),
            );
            buttons = buttons
                .child(ghost_button("export-svg", "Export SVG").on_click(cx.listener(|this, _, _, cx| {
                    this.export_svg(cx);
                    cx.notify();
                })))
                .child(ghost_button("copy-mermaid", "Copy Mermaid").on_click(cx.listener(|this, _, _, cx| {
                    this.copy_mermaid(cx);
                    cx.notify();
                })));
            let plural = |n: usize| if n == 1 { "" } else { "s" };
            let shown = self.view.nodes.len();
            let stats = match &self.focus_file {
                Some(_) if self.entry_count == 0 => "No components, hooks or functions in this file".to_string(),
                Some(_) => format!(
                    "{} symbol{} in this file · {} node{} shown · faded = other files",
                    self.entry_count,
                    plural(self.entry_count),
                    shown,
                    plural(shown),
                ),
                None => format!(
                    "{} entry point{} · {} node{} shown{}",
                    self.entry_count,
                    plural(self.entry_count),
                    shown,
                    plural(shown),
                    if self.isolated > 0 {
                        format!(" · {} unused components hidden", self.isolated)
                    } else {
                        String::new()
                    }
                ),
            };
            card()
                .id("toolbar")
                .occlude()
                .absolute()
                .top(px(MARGIN))
                .left(px(LEFT_W + MARGIN))
                .max_w(px((vw - LEFT_W - MARGIN - right_gap).max(220.)))
                .px_3()
                .py_2()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(crumbs)
                        .child(buttons),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_3()
                        .child(div().text_xs().text_color(rgb(MUTED)).child(stats))
                        .child(legend_dot(kind_color(SymKind::Component), "component"))
                        .child(legend_dot(kind_color(SymKind::Hook), "hook"))
                        .child(legend_dot(kind_color(SymKind::Function), "function")),
                )
        });

        // ---- right detail panel ----
        let detail = selected.map(|sel| {
            let s = &self.graph.symbols[sel];
            let kids = self.children(sel).len();
            let open = self.expanded.contains(&sel);
            let mut p = card()
                .id("detail")
                .occlude()
                .absolute()
                .top(px(MARGIN))
                .right(px(MARGIN))
                .when(show_code, |d| d.w(px(self.code_w(vw))).h(px(vh - 2.0 * MARGIN)))
                .when(!show_code, |d| d.w(px(DETAIL_W)).max_h(px(vh - 3.0 * MARGIN - MM_H)).overflow_y_scroll())
                .p_4()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(kind_color(s.kind)))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_family(MONO_FONT)
                                        .text_color(rgb(MUTED))
                                        .child(kind_name(s.kind)),
                                ),
                        )
                        .child(
                            div()
                                .id("close")
                                .px_2()
                                .rounded(px(6.))
                                .cursor_pointer()
                                .text_color(rgb(MUTED))
                                .hover(|st| st.bg(rgb(0xf3f4f6)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.selected = None;
                                    cx.notify();
                                }))
                                .child("✕"),
                        ),
                )
                .child(div().text_xl().font_weight(gpui::FontWeight::BOLD).child(s.name.clone()))
                .child(div().text_xs().text_color(rgb(MUTED)).child(if s.end_line > s.line {
                    format!("{}:{}–{}", s.file, s.line, s.end_line)
                } else {
                    format!("{}:{}", s.file, s.line)
                }))
                .child(tab_bar(show_code, cx));
            if show_code {
                return p.child(self.code_block(code_range, cx));
            }
            if self.focus_file.as_deref() != Some(s.file.as_str()) {
                let file = s.file.clone();
                p = p.child(
                    div().flex().flex_row().child(
                        ghost_button("open-sel-file", format!("Show only {}", short_file(&s.file)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_file(Some(file.clone()));
                                // narrow to the file, keep this symbol picked inside it
                                this.pick_from_list(sel);
                                cx.notify();
                            })),
                    ),
                );
            }
            if let Some(par) = s.parent {
                p = p.child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(format!("defined inside {}", self.graph.symbols[par].name)),
                );
            }
            let flowing = self.flow.is_some();
            p = p.child(
                div()
                    .mt_2()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .when(kids > 0, |d| {
                        d.child(
                            primary_button("expand-sel", if open { "Collapse".to_string() } else { format!("Expand ({kids})") })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_expand(sel);
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        ghost_button("play-flow", if flowing { "■ Stop flow" } else { "▶ Play flow" })
                            .text_sm()
                            .py(px(5.))
                            .px_3()
                            .text_color(rgb(ACCENT))
                            .border_color(rgb(ACCENT_300))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.flow.is_some() {
                                    this.flow = None;
                                } else {
                                    this.start_flow();
                                }
                                cx.notify();
                            })),
                    ),
            );

            if !s.wraps.is_empty() {
                p = p.child(section("Memoization"));
                for w in &s.wraps {
                    let (fg, bg) = wrap_colors(*w);
                    p = p.child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_start()
                            .child(
                                div()
                                    .px(px(5.))
                                    .rounded(px(4.))
                                    .bg(rgb(bg))
                                    .text_xs()
                                    .font_family(MONO_FONT)
                                    .text_color(rgb(fg))
                                    .child(w.label()),
                            )
                            .child(div().flex_1().min_w(px(0.)).text_xs().text_color(rgb(MUTED)).child(wrap_explainer(*w))),
                    );
                }
            }
            if let Some(info) = &s.deps {
                p = p.child(section("Dependencies")).child(dep_block(info));
            }
            if !s.dep_hooks.is_empty() {
                p = p.child(section("Hooks with dependencies"));
                for (n, info) in s.dep_hooks.iter().enumerate() {
                    let line = info.line;
                    p = p.child(
                        div()
                            .mt_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .id(("dep-hook", n))
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .items_center()
                                    .cursor_pointer()
                                    .rounded(px(6.))
                                    .hover(|st| st.bg(rgb(ACCENT_50)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let sym = &this.graph.symbols[sel];
                                        let (file, start) = (sym.file.clone(), sym.line);
                                        this.source_tab = true;
                                        this.ensure_code(&file, start);
                                        this.code.scroll.scroll_to_item_strict(line.saturating_sub(4), ScrollStrategy::Top);
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_family(MONO_FONT)
                                            .text_color(kind_color(SymKind::Hook))
                                            .child(info.hook.clone()),
                                    )
                                    .child(div().text_xs().text_color(rgb(MUTED)).child(format!("line {line}"))),
                            )
                            .child(dep_block(info)),
                    );
                }
            }

            if !s.prop_hints.is_empty() {
                p = p.child(section("Props defeating memo"));
                for (n, h) in s.prop_hints.iter().enumerate() {
                    let (line, child) = (h.line, h.child);
                    p = p.child(
                        div()
                            .id(("prop-hint", n))
                            .px_2()
                            .py_1()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|st| st.bg(rgb(ACCENT_50)))
                            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                                if e.modifiers().control || e.modifiers().platform {
                                    this.reveal(child);
                                } else {
                                    let sym = &this.graph.symbols[sel];
                                    let (file, start) = (sym.file.clone(), sym.line);
                                    this.source_tab = true;
                                    this.ensure_code(&file, start);
                                    this.code.scroll.scroll_to_item_strict(line.saturating_sub(4), ScrollStrategy::Top);
                                }
                                cx.notify();
                            }))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .items_center()
                                    .child(div().text_xs().text_color(rgb(0xb45309)).child("⚠"))
                                    .child(div().text_sm().font_family(MONO_FONT).child(h.prop.clone()))
                                    .child(div().text_xs().text_color(rgb(MUTED)).child(format!("line {line}"))),
                            )
                            .child(div().pl(px(15.)).min_w(px(0.)).text_xs().text_color(rgb(0x92400e)).child(h.text.clone())),
                    );
                }
            }

            if !s.drill_hints.is_empty() {
                p = p.child(section("Prop drilling"));
                for (n, h) in s.drill_hints.iter().enumerate() {
                    let (line, end) = (h.line, h.end);
                    p = p.child(
                        div()
                            .id(("drill-hint", n))
                            .px_2()
                            .py_1()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|st| st.bg(rgb(ACCENT_50)))
                            .tooltip(tip("Show the line", Some(if MAC { "⌘-click: jump to where it is used" } else { "Ctrl+click: jump to where it is used" })))
                            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                                if e.modifiers().control || e.modifiers().platform {
                                    this.reveal(end);
                                } else {
                                    let sym = &this.graph.symbols[sel];
                                    let (file, start) = (sym.file.clone(), sym.line);
                                    this.source_tab = true;
                                    this.ensure_code(&file, start);
                                    this.code.scroll.scroll_to_item_strict(line.saturating_sub(4), ScrollStrategy::Top);
                                }
                                cx.notify();
                            }))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .items_center()
                                    .child(div().text_xs().text_color(rgb(0xb45309)).child("⚠"))
                                    .child(div().text_sm().font_family(MONO_FONT).child(h.prop.clone()))
                                    .child(div().text_xs().text_color(rgb(MUTED)).child(format!("line {line}"))),
                            )
                            .child(div().pl(px(15.)).min_w(px(0.)).text_xs().text_color(rgb(0x92400e)).child(h.text.clone())),
                    );
                }
            }

            let mut dups = Vec::new();
            self.similar_within(sel, &mut dups);
            if !dups.is_empty() {
                p = p.child(section("Similar code")).child(
                    div().text_xs().text_color(rgb(MUTED)).child(
                        "Same structure elsewhere (names and values ignored). \
                         Click one to compare it side by side.",
                    ),
                );
                let mut n: usize = 0;
                for owner in dups {
                    let o = &self.graph.symbols[owner];
                    if owner != sel {
                        p = p.child(
                            div()
                                .mt_1()
                                .text_xs()
                                .font_family(MONO_FONT)
                                .text_color(kind_color(o.kind))
                                .child(format!("{} (inside)", o.name)),
                        );
                    }
                    for m in o.similar.iter().take(10) {
                        let other = m.other;
                        let is_cmp = self.cmp.as_ref().and_then(|c| c.data.as_ref().ok()).is_some_and(|c| c.a == owner && c.b == other);
                        let t = &self.graph.symbols[other];
                        n += 1;
                        p = p.child(
                            div()
                                .id(("dup", n))
                                .px_2()
                                .py_1()
                                .rounded(px(6.))
                                .cursor_pointer()
                                .when(is_cmp, |d| d.bg(rgb(0xf5f3ff)))
                                .hover(|st| st.bg(rgb(0xf5f3ff)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_compare(owner, other);
                                    cx.notify();
                                }))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(div().flex_shrink_0().w(px(7.)).h(px(7.)).rounded_full().bg(kind_color(t.kind)))
                                        .child(div().text_sm().child(t.name.clone()))
                                        .child(
                                            div()
                                                .px(px(4.))
                                                .rounded(px(4.))
                                                .bg(rgb(DUP_BG))
                                                .text_xs()
                                                .text_color(rgb(DUP_FG))
                                                .child(if m.exact { "identical".to_string() } else { pct(m.score) }),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            div()
                                                .id(("dup-go", n))
                                                .px(px(4.))
                                                .rounded(px(4.))
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .hover(|st| st.text_color(rgb(ACCENT)))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.cmp = None;
                                                    this.reveal(other);
                                                    cx.notify();
                                                }))
                                                .child("Go to"),
                                        ),
                                )
                                .child(
                                    div()
                                        .pl(px(15.))
                                        .min_w(px(0.))
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .child(format!("{}:{}", t.file, t.line)),
                                ),
                        );
                    }
                }
            }

            let mut inside = Vec::new();
            self.warned_descendants(sel, &mut inside);
            if !inside.is_empty() {
                p = p.child(section("Warnings inside"));
                for (n, idx) in inside.into_iter().enumerate() {
                    let t = &self.graph.symbols[idx];
                    let first = t
                        .deps
                        .iter()
                        .chain(t.dep_hooks.iter())
                        .flat_map(|d| d.hints.iter())
                        .find(|h| h.warn)
                        .map(|h| h.text.clone())
                        .or_else(|| t.prop_hints.first().map(|h| h.text.clone()))
                        .or_else(|| t.drill_hints.first().map(|h| h.text.clone()))
                        .unwrap_or_default();
                    p = p.child(
                        div()
                            .id(("warn-in", n))
                            .px_2()
                            .py_1()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|st| st.bg(rgb(ACCENT_50)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.reveal(idx);
                                cx.notify();
                            }))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(div().flex_shrink_0().w(px(7.)).h(px(7.)).rounded_full().bg(kind_color(t.kind)))
                                    .child(div().text_sm().child(t.name.clone()))
                                    .child(
                                        div()
                                            .px(px(4.))
                                            .rounded(px(4.))
                                            .bg(rgb(0xfef3c7))
                                            .text_xs()
                                            .text_color(rgb(0xb45309))
                                            .child(format!("⚠ {}", t.warnings())),
                                    ),
                            )
                            .child(div().pl(px(15.)).min_w(px(0.)).text_xs().text_color(rgb(MUTED)).child(first)),
                    );
                }
            }

            if let Some(members) = self.graph.symbols[sel].cycle.map(|c| self.graph.cycles[c].clone()) {
                p = p.child(section("Cycle"));
                p = p.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(rgb(0xbe185d))
                        .child("Renders or calls itself again through these:"),
                );
                for (n, m) in members.into_iter().filter(|&m| m != sel).take(30).enumerate() {
                    let t = &self.graph.symbols[m];
                    p = p.child(
                        div()
                            .id(("cyc", n))
                            .px_2()
                            .py_1()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|st| st.bg(rgb(ACCENT_50)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.reveal(m);
                                cx.notify();
                            }))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().w(px(7.)).h(px(7.)).rounded_full().bg(kind_color(t.kind)))
                            .child(div().text_sm().child(t.name.clone())),
                    );
                }
            }

            p = p.child(section("Used by"));
            let used_by: Vec<_> = self.graph.in_edges(sel).take(30).copied().collect();
            if used_by.is_empty() {
                p = p.child(div().text_xs().text_color(rgb(MUTED)).child("Nothing — this is an entry point"));
            }
            for (n, e) in used_by.into_iter().enumerate() {
                let from = e.from;
                let t = &self.graph.symbols[from];
                p = p.child(
                    div()
                        .id(("by", n))
                        .px_2()
                        .py_1()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|st| st.bg(rgb(ACCENT_50)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.reveal(from);
                            cx.notify();
                        }))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(7.)).h(px(7.)).rounded_full().bg(kind_color(t.kind)))
                        .child(div().text_sm().child(t.name.clone()))
                        .child(div().text_xs().text_color(rgb(MUTED)).child(self.graph.edge_label(&e))),
                );
            }

            // while a flow plays: its steps, the current one highlighted; click one to jump there
            if let (Some(fv), Some(fr)) = (self.flow.as_ref(), frame.as_ref()) {
                let t = &fv.trace;
                p = p.child(section(match t.style {
                    Style::Waves => "Flow · waves",
                    Style::CallReturn => "Flow · call and return",
                }));
                let mut shown_later = false;
                for (n, (i, s)) in t.steps.iter().enumerate().filter(|(_, s)| s.dur > 0.0).take(300).enumerate() {
                    if s.later && !shown_later && t.style == Style::CallReturn {
                        shown_later = true;
                        p = p.child(div().mt_2().pl_2().text_xs().text_color(rgb(MUTED)).child("Later, on user events"));
                    }
                    let (cur, done) = (fr.current == Some(i), fv.t >= s.t0 + s.dur);
                    let (glyph, gcolor) = match s.mv {
                        Move::Go if s.recursion => ("↺", 0xb45309),
                        Move::Go => ("→", ACCENT),
                        Move::Back => ("←", TEAL),
                    };
                    let end = s.t0 + s.dur;
                    let site = &self.graph.symbols[s.edge.site];
                    p = p.child(
                        div()
                            .id(("fstep", n))
                            .pl(px(s.depth as f32 * 12. + 8.))
                            .pr_2()
                            .py(px(2.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .when(cur, |d| d.bg(rgb(ACCENT_50)))
                            .hover(|st| st.bg(rgb(ACCENT_50)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.flow_seek(end);
                                cx.notify();
                            }))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().w(px(12.)).flex_shrink_0().text_sm().text_color(rgb(gcolor)).child(glyph))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .text_sm()
                                    .text_color(rgb(if cur { ACCENT } else if done { INK } else { 0x9ca3af }))
                                    .child(t.describe(&self.graph, i)),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(rgb(0x9ca3af))
                                    .child(format!("{}:{}", short_file(&site.file), s.edge.line)),
                            ),
                    );
                }
                return p;
            }
            p = p.child(section("Flow"));
            for (n, (depth, idx, via, again)) in self.flow_list(sel).into_iter().enumerate().skip(1) {
                let t = &self.graph.symbols[idx];
                p = p.child(
                    div()
                        .id(("flow", n))
                        .pl(px((depth as f32 - 1.0) * 14. + 8.))
                        .pr_2()
                        .py(px(3.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|st| st.bg(rgb(ACCENT_50)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.reveal(idx);
                            cx.notify();
                        }))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x9ca3af))
                                .child(via.map(|v| v.label()).unwrap_or("")),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(kind_color(t.kind))
                                .child(if again { format!("{} ↑", t.name) } else { t.name.clone() }),
                        ),
                );
            }
            p
        });
        // whole-file source (toolbar "Source" button, nothing selected)
        let detail = detail.or_else(|| {
            let f = self.focus_file.clone().filter(|_| show_code)?;
            let (badge, color) = file_badge(&f);
            Some(
                card()
                    .id("detail")
                    .occlude()
                    .absolute()
                    .top(px(MARGIN))
                    .right(px(MARGIN))
                    .w(px(self.code_w(vw)))
                    .h(px(vh - 2.0 * MARGIN))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(rgb(color))
                                            .child(badge),
                                    )
                                    .child(div().text_xs().font_family(MONO_FONT).text_color(rgb(MUTED)).child("source")),
                            )
                            .child(
                                div()
                                    .id("close-src")
                                    .px_2()
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .text_color(rgb(MUTED))
                                    .hover(|st| st.bg(rgb(0xf3f4f6)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.file_panel = false;
                                        cx.notify();
                                    }))
                                    .child("✕"),
                            ),
                    )
                    .child(div().text_xl().font_weight(gpui::FontWeight::BOLD).child(short_file(&f).to_string()))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                        "{} · {} lines · click a line to select what is defined there",
                        f,
                        self.code.src.as_ref().and_then(|r| r.as_ref().ok()).map_or(0, |s| s.lines.len())
                    )))
                    .child(self.code_block(None, cx)),
            )
        });

        // ---- "Play flow" player (bottom, between the controls and the minimap) ----
        let player = self.flow.as_ref().zip(frame.as_ref()).map(|(fv, fr)| {
            let t = &fv.trace;
            let minimap_shown = has_graph && !show_code && self.cmp.is_none() && !seq_on;
            let x0 = LEFT_W + MARGIN + if seq_on { 0.0 } else { 26.0 + 16.0 };
            let x1 = vw - if minimap_shown { MM_W + 2.0 * MARGIN } else { self.right_gap(vw) } - 8.0;
            let bar_w = (x1 - x0).clamp(320.0, 780.0);
            let left = x0 + ((x1 - x0) - bar_w).max(0.0) / 2.0;
            // the progress track, after the card's padding (12) + border (1) and the transport buttons
            let track_x = left + 13.0 + TRANSPORT_W + TRACK_GAP;
            let (track_w, total) = (bar_w - 26.0 - TRANSPORT_W - 2.0 * TRACK_GAP - CLOSE_W, t.total);
            let seek = move |this: &mut GraphView, x: f32| this.flow_seek((x - track_x) / track_w * total);
            let seg = |id: &'static str, label: &'static str, on: bool| {
                div()
                    .id(id)
                    .px_2()
                    .py(px(2.))
                    .rounded(px(6.))
                    .text_xs()
                    .cursor_pointer()
                    .when(on, |d| d.bg(rgb(0xffffff)).text_color(rgb(ACCENT)).font_weight(gpui::FontWeight::SEMIBOLD).shadow_sm())
                    .when(!on, |d| d.text_color(rgb(MUTED)).hover(|s| s.text_color(rgb(INK))))
                    .child(label)
            };
            let group = || div().flex().flex_row().p(px(2.)).gap(px(2.)).rounded(px(8.)).bg(rgb(0xf1f3f5));
            let from_btn = |id: &'static str, label: &'static str, v: Start| {
                seg(id, label, self.flow_from == v).on_click(cx.listener(move |this, _, _, cx| {
                    this.flow_from = v;
                    this.start_flow();
                    cx.notify();
                }))
            };
            let style_btn = |id: &'static str, label: &'static str, v: Style| {
                seg(id, label, self.flow_style == v).on_click(cx.listener(move |this, _, _, cx| {
                    this.flow_style = v;
                    this.start_flow();
                    cx.notify();
                }))
            };
            let icon = |id: &'static str, label: &'static str| {
                div()
                    .id(id)
                    .min_w(px(26.))
                    .h(px(24.))
                    .px_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .text_sm()
                    .text_color(rgb(INK))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0xf3f4f6)).text_color(rgb(ACCENT)))
                    .child(label)
            };
            let speed = self.flow_speed;
            let steps_total = t.steps.iter().filter(|s| s.dur > 0.0).count();
            let step_no = fr.current.map_or(0, |c| t.steps[..=c].iter().filter(|s| s.dur > 0.0).count());
            let name = |i: usize| self.graph.symbols[i].name.clone();
            let caption = match fr.current {
                Some(i) => {
                    let s = &t.steps[i];
                    let site = &self.graph.symbols[s.edge.site];
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .min_w(px(0.))
                        .child(div().flex_shrink_0().text_xs().text_color(rgb(MUTED)).child(format!("{step_no}/{steps_total}")))
                        .when(s.later, |d| {
                            d.child(
                                div()
                                    .flex_shrink_0()
                                    .px(px(5.))
                                    .rounded(px(4.))
                                    .bg(rgb(0xdcfce7))
                                    .text_xs()
                                    .text_color(rgb(0x15803d))
                                    .child("on event"),
                            )
                        })
                        .child(div().min_w(px(0.)).truncate().text_sm().child(t.describe(&self.graph, i)))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child(format!("{}:{}", short_file(&site.file), s.edge.line)),
                        )
                }
                None => div().text_sm().text_color(rgb(MUTED)).child(format!(
                    "Starts at {}{}",
                    t.origins.iter().take(4).map(|&o| name(o)).collect::<Vec<_>>().join(", "),
                    if t.origins.len() > 4 { "…" } else { "" }
                )),
            };
            card()
                .id("flow-player")
                .occlude()
                .absolute()
                .left(px(left))
                .bottom(px(MARGIN))
                .w(px(bar_w))
                .px_3()
                .py_2()
                .flex()
                .flex_col()
                .gap(px(6.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            group()
                                .child(from_btn("flow-here", "From here", Start::Here))
                                .child(from_btn("flow-parent", "From parent", Start::Parent))
                                .child(from_btn("flow-entry", "From entry", Start::Entry)),
                        )
                        .child(
                            group()
                                .child(style_btn("flow-calls", "Call & return", Style::CallReturn))
                                .child(style_btn("flow-waves", "Waves", Style::Waves)),
                        )
                        .child(
                            group()
                                .child(seg("flow-graph", "Graph", !self.flow_seq).on_click(cx.listener(|this, _, _, cx| {
                                    this.flow_seq = false;
                                    cx.notify();
                                })))
                                .child(seg("flow-seq", "Sequence", self.flow_seq).on_click(cx.listener(|this, _, _, cx| {
                                    this.flow_seq = true;
                                    this.seq_follow = true;
                                    cx.notify();
                                }))),
                        ),
                )
                // transport, progress (click or drag the track to scrub), close
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(TRACK_GAP))
                        .child(
                            div()
                                .w(px(TRANSPORT_W))
                                .flex_shrink_0()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(4.))
                                .child(icon("flow-prev", "‹").on_click(cx.listener(|this, _, _, cx| {
                                    this.flow_step(false);
                                    cx.notify();
                                })))
                                .child(icon("flow-play", if fv.playing { "❚❚" } else { "▶" }).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.flow_toggle();
                                        cx.notify();
                                    },
                                )))
                                .child(icon("flow-next", "›").on_click(cx.listener(|this, _, _, cx| {
                                    this.flow_step(true);
                                    cx.notify();
                                })))
                                .child(
                                    icon("flow-speed", if speed < 1.0 { "0.5×" } else if speed > 1.0 { "2×" } else { "1×" })
                                        .text_xs()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.flow_speed =
                                                if this.flow_speed < 1.0 { 1.0 } else if this.flow_speed < 2.0 { 2.0 } else { 0.5 };
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .id("flow-track")
                                .flex_1()
                                .h(px(16.))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                        seek(this, f32::from(ev.position.x));
                                        cx.notify();
                                    }),
                                )
                                .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                                    if ev.dragging() && this.flow.is_some() {
                                        seek(this, f32::from(ev.position.x));
                                        cx.notify();
                                    }
                                }))
                                .child(
                                    div().w_full().h(px(4.)).rounded_full().bg(rgb(0xf1f3f5)).child(
                                        div()
                                            .h_full()
                                            .rounded_full()
                                            .bg(rgb(ACCENT))
                                            .w(px(track_w * (fv.t / total).clamp(0.0, 1.0))),
                                    ),
                                ),
                        )
                        .child(
                            icon("flow-close", "✕")
                                .w(px(CLOSE_W))
                                .flex_shrink_0()
                                .text_color(rgb(MUTED))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.flow = None;
                                    cx.notify();
                                })),
                        ),
                )
                .child(caption.h(px(22.)))
                // last row, always there so the bar keeps its height (and its buttons stay put):
                // the call stack in call-and-return, a legend in waves
                .child({
                    let mut row = div()
                        .h(px(20.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_xs()
                        .text_color(rgb(MUTED));
                    match t.style {
                        Style::CallReturn => {
                            row = row.child(div().mr_1().child("Stack"));
                            let n = fr.stack.len();
                            let skip = n.saturating_sub(6);
                            if skip > 0 {
                                row = row.child(div().child("… ›"));
                            }
                            for (k, &sym) in fr.stack.iter().enumerate().skip(skip) {
                                let top = k + 1 == n;
                                // the same symbol lower down: this frame is a recursion
                                let again = fr.stack[..k].contains(&sym);
                                row = row.child(
                                    div()
                                        .px(px(5.))
                                        .rounded(px(4.))
                                        .font_family(MONO_FONT)
                                        .when(top, |d| d.bg(rgb(ACCENT_100)).text_color(rgb(ACCENT)))
                                        .when(!top, |d| d.bg(rgb(0xf3f4f6)).text_color(rgb(INK)))
                                        .child(if again { format!("{} ↺", name(sym)) } else { name(sym) }),
                                );
                                if !top {
                                    row = row.child(div().text_color(rgb(0xcbd5e1)).child("›"));
                                }
                            }
                        }
                        Style::Waves => {
                            row = row.child("Everything it reaches, one level at a time · handlers fire after the render");
                        }
                    }
                    if t.truncated {
                        row = row.child(div().flex_1()).child("· trace cut short");
                    }
                    row
                })
        });

        // ---- controls (bottom-left), like <Controls /> ----
        let controls = div()
            .id("controls")
            .occlude()
            .absolute()
            .left(px(LEFT_W + MARGIN))
            .bottom(px(MARGIN))
            .flex()
            .flex_col()
            .rounded(px(4.))
            .overflow_hidden()
            .border_1()
            .border_color(rgb(BORDER))
            .shadow_md()
            .child(control_button("zin", "+", false).on_click(cx.listener(|this, _, window, cx| {
                let vp = window.viewport_size();
                this.zoom_at(1.25, f32::from(vp.width) / 2.0, (f32::from(vp.height) - TB) / 2.0);
                cx.notify();
            })))
            .child(control_button("zout", "−", false).on_click(cx.listener(|this, _, window, cx| {
                let vp = window.viewport_size();
                this.zoom_at(1.0 / 1.25, f32::from(vp.width) / 2.0, (f32::from(vp.height) - TB) / 2.0);
                cx.notify();
            })))
            .child(control_button("fit", "⛶", true).on_click(cx.listener(|this, _, _, cx| {
                this.fit_pending = true;
                cx.notify();
            })));

        // ---- minimap (bottom-right), like <MiniMap /> ----
        let (mx0, my0, ms, mpx, mpy) = self.minimap_xform(vw, vh);
        let mm_nodes: Vec<(f32, f32, Hsla)> = self
            .view
            .nodes
            .iter()
            .filter_map(|i| {
                self.view.pos.get(i).map(|&(x, y)| {
                    let c: Hsla = if selected == Some(*i) {
                        rgb(ACCENT).into()
                    } else if focus.is_some() && related(*i) {
                        rgb(ACCENT_300).into()
                    } else {
                        rgb(0xe2e2e2).into()
                    };
                    (x, y, c)
                })
            })
            .collect();
        let vrect = (-ox / z, -oy / z, vw / z, vh / z);
        let minimap = div()
            .id("minimap")
            .occlude()
            .absolute()
            .right(px(MARGIN))
            .bottom(px(MARGIN))
            .w(px(MM_W))
            .h(px(MM_H))
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(BORDER))
            .rounded(px(6.))
            .shadow_md()
            .overflow_hidden()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    let vp = window.viewport_size();
                    this.mm_drag = true;
                    this.minimap_jump(
                        f32::from(ev.position.x),
                        f32::from(ev.position.y) - TB,
                        f32::from(vp.width),
                        f32::from(vp.height) - TB,
                    );
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                if this.mm_drag && ev.dragging() {
                    let vp = window.viewport_size();
                    this.minimap_jump(
                        f32::from(ev.position.x),
                        f32::from(ev.position.y) - TB,
                        f32::from(vp.width),
                        f32::from(vp.height) - TB,
                    );
                    cx.notify();
                } else {
                    this.mm_drag = false;
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.mm_drag = false))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let map = |x: f32, y: f32| {
                            bounds.origin + point(px(mpx + (x - mx0) * ms), px(mpy + (y - my0) * ms))
                        };
                        // dim everything, then punch out the viewport
                        window.paint_quad(fill(bounds, hsla(0xf0f0f0, 0.6)));
                        let (vx, vy, vw_, vh_) = vrect;
                        let vb = Bounds::new(map(vx, vy), size(px(vw_ * ms), px(vh_ * ms)));
                        window.paint_quad(fill(vb, hsla(0xffffff, 1.0)));
                        for (x, y, c) in &mm_nodes {
                            let b = Bounds::new(map(*x, *y), size(px((NODE_W * ms).max(2.0)), px((NODE_H * ms).max(1.5))));
                            window.paint_quad(fill(b, *c).corner_radii(px(2.0)));
                        }
                        let mut pb = PathBuilder::stroke(px(1.0));
                        let (a, b) = (vb.origin, vb.origin + point(vb.size.width, vb.size.height));
                        pb.move_to(a);
                        pb.line_to(point(b.x, a.y));
                        pb.line_to(b);
                        pb.line_to(point(a.x, b.y));
                        pb.close();
                        if let Ok(path) = pb.build() {
                            window.paint_path(path, hsla(ACCENT, 0.7));
                        }
                    },
                )
                .size_full(),
            );

        // ---- assemble ----
        let mut root = div()
            .relative()
            .w_full()
            .flex_1()
            .min_h(px(0.))
            .overflow_hidden()
            .bg(rgb(CANVAS))
            .font_family(UI_FONT)
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| window.focus(&this.focus)),
            )
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                let k = &ev.keystroke;
                let cmd = if MAC { k.modifiers.platform } else { k.modifiers.control };
                if cmd && !k.modifiers.alt && !k.modifiers.shift && k.key == "o" {
                    Self::prompt_open(cx);
                    return;
                }
                if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
                    return;
                }
                // flow player: space plays / pauses, arrows step (unless typing a search)
                if this.flow.is_some() && this.query.is_empty() {
                    match k.key.as_str() {
                        "space" => this.flow_toggle(),
                        "left" => this.flow_step(false),
                        "right" => this.flow_step(true),
                        _ => {}
                    }
                    if matches!(k.key.as_str(), "space" | "left" | "right") {
                        cx.notify();
                        return;
                    }
                }
                match k.key.as_str() {
                    "backspace" => {
                        this.query.pop();
                    }
                    "escape" => {
                        if this.menu.is_some() {
                            this.menu = None;
                        } else if this.flow.is_some() && this.query.is_empty() {
                            this.flow = None;
                        } else if this.cmp.is_some() {
                            this.cmp = None;
                        } else if this.query.is_empty() {
                            if this.selected.is_some() {
                                this.selected = None;
                            } else {
                                this.file_panel = false;
                            }
                        } else {
                            this.query.clear();
                        }
                    }
                    "enter" => {
                        if let Some(s) = this.selected {
                            this.toggle_expand(s);
                        }
                    }
                    _ => match &k.key_char {
                        Some(c) if !c.chars().any(|ch| ch.is_control()) => this.query.push_str(c),
                        _ => return,
                    },
                }
                cx.notify();
            }))
            .child(area);
        if let Some(fr) = frame.as_ref().filter(|_| seq_on) {
            root = root.child(self.sequence_view(vw, vh, fr, cx));
        }

        if self.loading {
            root = root.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .left(px(LEFT_W))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(card().px_4().py_3().text_sm().text_color(rgb(MUTED)).child("Reading and parsing files…")),
            );
        } else if self.root.is_empty() {
            root = root.child(self.welcome(cx));
        }

        // no project: just the welcome screen, like an IDE's start page
        if !self.root.is_empty() {
            root = root.child(side);
        }
        if let Some(t) = toolbar {
            root = root.child(t);
        }
        if let Some(c) = self.compare_panel(vw, vh, cx) {
            root = root.child(c);
        } else if let Some(d) = detail {
            root = root.child(d);
        }
        // the code panel takes the full height on the right: the minimap would sit on top of it
        if has_graph && !show_code && self.cmp.is_none() && !seq_on {
            root = root.child(minimap);
        }
        if let Some(p) = player {
            root = root.child(p);
        }
        if !seq_on && !self.root.is_empty() {
            root = root.child(controls);
        }
        if let Some(m) = self.card_menu(cx) {
            root = root.child(m);
        }
        if let Some(m) = self.project_popup(cx) {
            root = root.child(m);
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .font_family(UI_FONT)
            .text_color(rgb(INK))
            .bg(rgb(CANVAS))
            .child(self.title_bar(window, cx))
            .child(root)
    }
}

fn main() {
    let root = std::env::args().nth(1).map(PathBuf::from);

    Application::new().with_assets(Assets).run(move |cx: &mut App| {
        // macOS: an app menu with Quit (⌘Q), and quit when the window closes
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([gpui::KeyBinding::new("cmd-q", Quit, None)]);
        cx.set_menus(vec![gpui::Menu { name: "React Graph".into(), items: vec![gpui::MenuItem::action("Quit React Graph", Quit)] }]);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        // 1280×800, but never bigger than 90% of the screen (macOS shrinks oversized windows,
        // and the first frame then renders at the wrong size)
        let screen = cx.primary_display().map(|d| d.bounds().size).unwrap_or(size(px(1280.), px(800.)));
        let bounds = Bounds::centered(None, size(px(1280.).min(screen.width * 0.9), px(800.).min(screen.height * 0.9)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // no native caption: the title bar is drawn by the app (logo, project widget, window buttons)
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("React Graph".into()),
                    appears_transparent: true,
                    // centre macOS's traffic lights in the 40px title bar
                    traffic_light_position: Some(point(px(14.), px(14.))),
                }),
                app_id: Some("react-graph".into()),
                ..Default::default()
            },
            move |window, cx| {
                let is_dark = |w: &Window| matches!(w.appearance(), gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark);
                let system_dark = is_dark(window);
                let view = cx.new(|cx| {
                    let mut v = GraphView::empty(cx);
                    v.init_theme(system_dark);
                    // follow the OS while the user hasn't picked light or dark
                    cx.observe_window_appearance(window, move |this, window, cx| {
                        if this.prefs.theme.is_empty() {
                            set_dark(is_dark(window));
                            cx.notify();
                        }
                    })
                    .detach();
                    match &root {
                        Some(r) => v.load_folder(std::fs::canonicalize(r).unwrap_or(r.clone()), cx),
                        None => v.status = "No project loaded.".into(),
                    }
                    v
                });
                let h = view.read(cx).focus.clone();
                window.focus(&h);
                view
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn watcher_reports_source_changes_only() {
        let dir = std::env::temp_dir().join(format!("react-graph-watch-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let (_w, rx) = watch_folder(&dir).expect("watch the folder");

        // not a graph input: nothing arrives
        std::fs::write(dir.join("notes.md"), "hi").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(1500)).is_err());

        // a new component does
        std::fs::write(dir.join("src").join("New.tsx"), "export function New() { return <div /> }").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_ok(), "no event for a new .tsx file");

        std::fs::remove_dir_all(&dir).ok();
    }
}
