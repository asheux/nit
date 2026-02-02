use crate::theme::Theme;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
    Frame,
};
use std::{
    cmp::Reverse,
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
};

// Circuit renderer version for cache invalidation.
const STYLE_VERSION: u64 = 4;

// Keep both directions around for a potential future toggle.
#[allow(dead_code)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum FlowDir {
    LeftToRight,
    TopToBottom,
}

// Default circuit flow direction.
const FLOW_DIR: FlowDir = FlowDir::TopToBottom;

const MIN_AREA_W: u16 = 18;
const MIN_AREA_H: u16 = 7;

const MAX_CACHE_ENTRIES: usize = 16;

// Node box sizing.
const NODE_H: u16 = 3;
const NODE_MIN_W: u16 = 5;
const NODE_PAD_X: u16 = 1;

// Layout margins inside the provided area.
const MARGIN_X: u16 = 2;
const MARGIN_Y: u16 = 1;

// Routing costs.
const COST_STEP: u32 = 10;
const COST_WIRE_OVERLAP: u32 = 35;

#[derive(Clone, Debug)]
pub struct GraphSpec {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub start: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct GraphNode {
    pub id: usize,
    pub label: String,
    pub output: Option<char>,
}

#[derive(Clone, Debug)]
pub struct GraphEdge {
    pub from: usize,
    pub to: usize,
    pub label: String,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    graph_hash: u64,
    width: u16,
    height: u16,
    style_version: u64,
}

#[derive(Default)]
struct GraphRenderCache {
    entries: Vec<(CacheKey, Arc<CachedCircuitGraph>)>,
}

impl GraphRenderCache {
    fn get(&mut self, key: CacheKey) -> Option<Arc<CachedCircuitGraph>> {
        self.entries
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| Arc::clone(v))
    }

    fn insert(&mut self, key: CacheKey, graph: Arc<CachedCircuitGraph>) {
        self.entries.retain(|(k, _)| *k != key);
        self.entries.push((key, graph));
        if self.entries.len() > MAX_CACHE_ENTRIES {
            let overflow = self.entries.len().saturating_sub(MAX_CACHE_ENTRIES);
            self.entries.drain(0..overflow);
        }
    }
}

static GRAPH_RENDER_CACHE: OnceLock<Mutex<GraphRenderCache>> = OnceLock::new();

fn cache() -> &'static Mutex<GraphRenderCache> {
    GRAPH_RENDER_CACHE.get_or_init(|| Mutex::new(GraphRenderCache::default()))
}

#[derive(Clone, Debug)]
struct CachedCircuitGraph {
    width: u16,
    height: u16,
    message: Option<String>,

    nodes: Vec<CachedNode>,

    // Per-cell wire connectivity (U/R/D/L bits).
    wire_mask: Vec<u8>,
    // Per-cell wire color kind.
    wire_color: Vec<u8>,

    arrows: Vec<CachedArrow>,
    labels: Vec<CachedLabel>,

    start_arrow: Option<CachedArrow>,
}

#[derive(Clone, Debug)]
struct CachedNode {
    x: u16,
    y: u16,
    w: u16,
    label: String,
    is_start: bool,
    is_halt: bool,
    // Keep for future; circuit renderer doesn't currently use output for color.
    #[allow(dead_code)]
    output: Option<char>,
}

#[derive(Copy, Clone, Debug)]
struct CachedArrow {
    x: u16,
    y: u16,
    ch: char,
    color_kind: u8,
}

#[derive(Clone, Debug)]
struct CachedLabel {
    x: u16,
    y: u16,
    text: String,
    color_kind: u8,
}

struct CircuitWidget<'a> {
    cached: &'a CachedCircuitGraph,
    theme: &'a Theme,
}

impl Widget for CircuitWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Always clear the graph panel area.
        buf.set_style(area, Style::default().bg(self.theme.background));

        if area.width == 0 || area.height == 0 {
            return;
        }

        if let Some(msg) = self.cached.message.as_ref() {
            let style = Style::default()
                .fg(self.theme.warning)
                .add_modifier(Modifier::BOLD);
            let x = area.x + 1;
            let y = area.y + area.height / 2;
            buf.set_stringn(x, y, msg, area.width.saturating_sub(2) as usize, style);
            return;
        }

        // Wires.
        for y in 0..self.cached.height {
            for x in 0..self.cached.width {
                let idx = grid_idx(self.cached.width, x, y);
                let mask = self.cached.wire_mask[idx];
                if mask == 0 {
                    continue;
                }
                let ch = wire_char_from_mask(mask);
                let color = wire_color_from_kind(self.cached.wire_color[idx], self.theme);
                let gx = area.x + x;
                let gy = area.y + y;
                buf.get_mut(gx, gy)
                    .set_char(ch)
                    .set_style(Style::default().fg(color).bg(self.theme.background));
            }
        }

        // Arrowheads.
        for a in &self.cached.arrows {
            if a.x >= self.cached.width || a.y >= self.cached.height {
                continue;
            }
            let color = wire_color_from_kind(a.color_kind, self.theme);
            buf.get_mut(area.x + a.x, area.y + a.y)
                .set_char(a.ch)
                .set_style(
                    Style::default()
                        .fg(color)
                        .bg(self.theme.background)
                        .add_modifier(Modifier::BOLD),
                );
        }

        if let Some(a) = self.cached.start_arrow {
            if a.x < self.cached.width && a.y < self.cached.height {
                buf.get_mut(area.x + a.x, area.y + a.y)
                    .set_char(a.ch)
                    .set_style(
                        Style::default()
                            .fg(self.theme.accent)
                            .bg(self.theme.background)
                            .add_modifier(Modifier::BOLD),
                    );
            }
        }

        // Labels on top of wires.
        for label in &self.cached.labels {
            let style = Style::default()
                .fg(wire_color_from_kind(label.color_kind, self.theme))
                .bg(self.theme.cursor_line_bg)
                .add_modifier(Modifier::BOLD);
            buf.set_stringn(
                area.x + label.x,
                area.y + label.y,
                &label.text,
                self.cached.width.saturating_sub(label.x) as usize,
                style,
            );
        }

        // Nodes on top.
        for node in &self.cached.nodes {
            let border = if node.is_start {
                self.theme.border_focused
            } else if node.is_halt {
                self.theme.warning
            } else if node.output == Some('C') {
                self.theme.title
            } else if node.output == Some('D') {
                self.theme.accent
            } else {
                self.theme.border
            };
            let fill = if node.is_start {
                self.theme.selection_bg
            } else if node.output == Some('D') {
                self.theme.selection_bg
            } else {
                self.theme.cursor_line_bg
            };
            draw_node_box(buf, area, node, border, fill, self.theme);
        }
    }
}

pub fn render(frame: &mut Frame, area: Rect, theme: &Theme, graph: &GraphSpec) {
    let graph_hash = graph_hash(graph);
    let key = CacheKey {
        graph_hash,
        width: area.width,
        height: area.height,
        style_version: STYLE_VERSION,
    };

    let cached = {
        let mut guard = cache().lock().expect("graph render cache poisoned");
        if let Some(hit) = guard.get(key) {
            hit
        } else {
            drop(guard);
            let computed = Arc::new(compute_circuit_graph(graph, area, graph_hash));
            let mut guard = cache().lock().expect("graph render cache poisoned");
            guard.insert(key, Arc::clone(&computed));
            computed
        }
    };

    frame.render_widget(
        CircuitWidget {
            cached: cached.as_ref(),
            theme,
        },
        area,
    );
}

fn compute_circuit_graph(graph: &GraphSpec, area: Rect, _graph_hash: u64) -> CachedCircuitGraph {
    let width = area.width;
    let height = area.height;

    if width < MIN_AREA_W || height < MIN_AREA_H {
        return CachedCircuitGraph {
            width,
            height,
            message: Some("graph panel too small".to_string()),
            nodes: Vec::new(),
            wire_mask: vec![0; (width as usize) * (height as usize)],
            wire_color: vec![0; (width as usize) * (height as usize)],
            arrows: Vec::new(),
            labels: Vec::new(),
            start_arrow: None,
        };
    }

    let node_w = compute_node_width(graph);
    let node_h = NODE_H;

    // Assign BFS layers from the start state; unreachable nodes go to the last layer.
    let layers = assign_layers(graph);
    let mut max_layer = 0usize;
    for &l in layers.values() {
        max_layer = max_layer.max(l);
    }
    let layer_count = max_layer.saturating_add(1).max(1);

    // Group nodes by layer.
    let mut nodes_by_layer: Vec<Vec<&GraphNode>> = vec![Vec::new(); layer_count];
    for node in &graph.nodes {
        let layer = *layers.get(&node.id).unwrap_or(&max_layer);
        if let Some(slot) = nodes_by_layer.get_mut(layer) {
            slot.push(node);
        }
    }
    for layer in &mut nodes_by_layer {
        layer.sort_by_key(|n| n.id);
    }

    let max_per_layer = nodes_by_layer
        .iter()
        .map(|v| v.len())
        .max()
        .unwrap_or(0)
        .max(1);

    let (col_gap, row_gap, needed_w, needed_h) = match FLOW_DIR {
        FlowDir::LeftToRight => {
            let col_gap = compute_gap(width, layer_count as u16, node_w, MARGIN_X);
            let row_gap = compute_gap(height, max_per_layer as u16, node_h, MARGIN_Y);
            let needed_w = MARGIN_X * 2
                + (layer_count as u16) * node_w
                + (layer_count as u16).saturating_sub(1) * col_gap;
            let needed_h = MARGIN_Y * 2
                + (max_per_layer as u16) * node_h
                + (max_per_layer as u16).saturating_sub(1) * row_gap;
            (col_gap, row_gap, needed_w, needed_h)
        }
        FlowDir::TopToBottom => {
            let col_gap = compute_gap(width, max_per_layer as u16, node_w, MARGIN_X);
            let row_gap = compute_gap(height, layer_count as u16, node_h, MARGIN_Y);
            let needed_w = MARGIN_X * 2
                + (max_per_layer as u16) * node_w
                + (max_per_layer as u16).saturating_sub(1) * col_gap;
            let needed_h = MARGIN_Y * 2
                + (layer_count as u16) * node_h
                + (layer_count as u16).saturating_sub(1) * row_gap;
            (col_gap, row_gap, needed_w, needed_h)
        }
    };
    if needed_w > width || needed_h > height {
        return CachedCircuitGraph {
            width,
            height,
            message: Some("graph does not fit".to_string()),
            nodes: Vec::new(),
            wire_mask: vec![0; (width as usize) * (height as usize)],
            wire_color: vec![0; (width as usize) * (height as usize)],
            arrows: Vec::new(),
            labels: Vec::new(),
            start_arrow: None,
        };
    }

    let mut nodes: Vec<CachedNode> = Vec::with_capacity(graph.nodes.len());
    let mut node_index: HashMap<usize, usize> = HashMap::new();

    match FLOW_DIR {
        FlowDir::LeftToRight => {
            // Slot Y positions shared across layers to reduce crossings.
            let x0 = MARGIN_X;
            let y0 = centered_start(height, max_per_layer as u16, node_h, MARGIN_Y, row_gap);
            let mut slot_y: Vec<u16> = Vec::with_capacity(max_per_layer);
            for idx in 0..max_per_layer {
                let y = y0 + idx as u16 * (node_h + row_gap);
                slot_y.push(y);
            }

            for (layer_idx, layer_nodes) in nodes_by_layer.iter().enumerate() {
                let x = x0 + layer_idx as u16 * (node_w + col_gap);
                let layer_len = layer_nodes.len();
                let offset = (max_per_layer.saturating_sub(layer_len)) / 2;
                for (pos_idx, node) in layer_nodes.iter().enumerate() {
                    let y = slot_y[offset + pos_idx];
                    let is_halt = node.label == "H";
                    let is_start = graph.start == Some(node.id);
                    let cached = CachedNode {
                        x,
                        y,
                        w: node_w,
                        label: node.label.clone(),
                        is_start,
                        is_halt,
                        output: node.output,
                    };
                    node_index.insert(node.id, nodes.len());
                    nodes.push(cached);
                }
            }
        }
        FlowDir::TopToBottom => {
            // Slot X positions shared across layers to reduce crossings.
            let x0 = centered_start(width, max_per_layer as u16, node_w, MARGIN_X, col_gap);
            let y0 = MARGIN_Y;
            let mut slot_x: Vec<u16> = Vec::with_capacity(max_per_layer);
            for idx in 0..max_per_layer {
                let x = x0 + idx as u16 * (node_w + col_gap);
                slot_x.push(x);
            }

            for (layer_idx, layer_nodes) in nodes_by_layer.iter().enumerate() {
                let y = y0 + layer_idx as u16 * (node_h + row_gap);
                let layer_len = layer_nodes.len();
                let offset = (max_per_layer.saturating_sub(layer_len)) / 2;
                for (pos_idx, node) in layer_nodes.iter().enumerate() {
                    let x = slot_x[offset + pos_idx];
                    let is_halt = node.label == "H";
                    let is_start = graph.start == Some(node.id);
                    let cached = CachedNode {
                        x,
                        y,
                        w: node_w,
                        label: node.label.clone(),
                        is_start,
                        is_halt,
                        output: node.output,
                    };
                    node_index.insert(node.id, nodes.len());
                    nodes.push(cached);
                }
            }
        }
    }

    // Blocked cells (nodes).
    let mut blocked = vec![false; (width as usize) * (height as usize)];
    for node in &nodes {
        for dy in 0..node_h {
            for dx in 0..node.w {
                let x = node.x + dx;
                let y = node.y + dy;
                if x < width && y < height {
                    blocked[grid_idx(width, x, y)] = true;
                }
            }
        }
    }

    // Port offsets so parallel edges don't stack on top of each other.
    let mut out_edges: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut in_edges: HashMap<usize, Vec<usize>> = HashMap::new();
    for (idx, e) in graph.edges.iter().enumerate() {
        out_edges.entry(e.from).or_default().push(idx);
        in_edges.entry(e.to).or_default().push(idx);
    }
    for (_node_id, idxs) in out_edges.iter_mut() {
        idxs.sort_by(|&a, &b| {
            let ea = &graph.edges[a];
            let eb = &graph.edges[b];
            let la = *layers.get(&ea.to).unwrap_or(&usize::MAX);
            let lb = *layers.get(&eb.to).unwrap_or(&usize::MAX);
            la.cmp(&lb)
                .then_with(|| ea.to.cmp(&eb.to))
                .then_with(|| ea.label.cmp(&eb.label))
        });
    }
    for (_node_id, idxs) in in_edges.iter_mut() {
        idxs.sort_by(|&a, &b| {
            let ea = &graph.edges[a];
            let eb = &graph.edges[b];
            let la = *layers.get(&ea.from).unwrap_or(&usize::MAX);
            let lb = *layers.get(&eb.from).unwrap_or(&usize::MAX);
            la.cmp(&lb)
                .then_with(|| ea.from.cmp(&eb.from))
                .then_with(|| ea.label.cmp(&eb.label))
        });
    }

    let mut out_port: Vec<i16> = vec![0; graph.edges.len()];
    let mut in_port: Vec<i16> = vec![0; graph.edges.len()];
    for idxs in out_edges.values() {
        for (k, &edge_idx) in idxs.iter().enumerate() {
            out_port[edge_idx] = port_offset(k);
        }
    }
    for idxs in in_edges.values() {
        for (k, &edge_idx) in idxs.iter().enumerate() {
            in_port[edge_idx] = port_offset(k);
        }
    }

    // Wire grids.
    let mut wire_mask = vec![0u8; (width as usize) * (height as usize)];
    let mut wire_color = vec![0u8; (width as usize) * (height as usize)];
    let mut wire_used = vec![0u8; (width as usize) * (height as usize)];

    // Deterministic edge routing order.
    let mut edge_order: Vec<usize> = (0..graph.edges.len()).collect();
    edge_order.sort_by(|&a, &b| {
        let ea = &graph.edges[a];
        let eb = &graph.edges[b];
        let la = *layers.get(&ea.from).unwrap_or(&max_layer);
        let lb = *layers.get(&eb.from).unwrap_or(&max_layer);
        let ta = *layers.get(&ea.to).unwrap_or(&max_layer);
        let tb = *layers.get(&eb.to).unwrap_or(&max_layer);
        la.cmp(&lb)
            .then_with(|| ta.cmp(&tb))
            .then_with(|| ea.from.cmp(&eb.from))
            .then_with(|| ea.to.cmp(&eb.to))
            .then_with(|| ea.label.cmp(&eb.label))
    });

    let mut arrows = Vec::with_capacity(graph.edges.len());
    let mut labels_out = Vec::with_capacity(graph.edges.len());

    for edge_idx in edge_order {
        let e = &graph.edges[edge_idx];
        let Some(&from_i) = node_index.get(&e.from) else {
            continue;
        };
        let Some(&to_i) = node_index.get(&e.to) else {
            continue;
        };
        let from_node = &nodes[from_i];
        let to_node = &nodes[to_i];

        let (start, end, edge_flow) = match FLOW_DIR {
            FlowDir::LeftToRight => (
                anchor_right(from_node, out_port[edge_idx], width, height),
                anchor_left(to_node, in_port[edge_idx], width, height),
                FlowDir::LeftToRight,
            ),
            FlowDir::TopToBottom => {
                let from_layer = *layers.get(&e.from).unwrap_or(&max_layer);
                let to_layer = *layers.get(&e.to).unwrap_or(&max_layer);
                if e.from == e.to {
                    (
                        anchor_bottom(from_node, out_port[edge_idx], width, height),
                        anchor_top(to_node, in_port[edge_idx], width, height),
                        // Encourage a side-loop rather than a tight vertical hop.
                        FlowDir::LeftToRight,
                    )
                } else if to_layer > from_layer {
                    (
                        anchor_bottom(from_node, out_port[edge_idx], width, height),
                        anchor_top(to_node, in_port[edge_idx], width, height),
                        FlowDir::TopToBottom,
                    )
                } else if to_layer < from_layer {
                    (
                        anchor_top(from_node, out_port[edge_idx], width, height),
                        anchor_bottom(to_node, in_port[edge_idx], width, height),
                        FlowDir::TopToBottom,
                    )
                } else if to_node.x >= from_node.x {
                    (
                        anchor_right(from_node, out_port[edge_idx], width, height),
                        anchor_left(to_node, in_port[edge_idx], width, height),
                        FlowDir::LeftToRight,
                    )
                } else {
                    (
                        anchor_left(from_node, out_port[edge_idx], width, height),
                        anchor_right(to_node, in_port[edge_idx], width, height),
                        FlowDir::LeftToRight,
                    )
                }
            }
        };

        let color_kind = edge_color_kind(&e.label);

        // Route path. Self-loops are routed just like other edges (with a left-side endpoint) so
        // they appear as a feedback wire.
        let path = route_path(start, end, width, height, &blocked, &wire_used, edge_flow);
        let Some(path) = path else {
            continue;
        };

        apply_wire_path(
            &path,
            width,
            &mut wire_mask,
            &mut wire_color,
            &mut wire_used,
            color_kind,
        );

        // Arrowhead at the end.
        if path.len() >= 2 {
            let a = path[path.len() - 2];
            let b = path[path.len() - 1];
            let arrow = CachedArrow {
                x: b.0,
                y: b.1,
                ch: arrow_char(a, b),
                color_kind,
            };
            arrows.push(arrow);
        }

        // Label chip.
        let chip = format!("[{}]", e.label);
        let mid = path[path.len() / 2];
        let chip_x = chip_anchor_x(mid.0, width, chip.len() as u16);
        labels_out.push(CachedLabel {
            x: chip_x,
            y: mid.1,
            text: chip,
            color_kind,
        });
    }

    // Start arrow into the start node, coming from "nowhere".
    let start_arrow = graph.start.and_then(|id| {
        let &idx = node_index.get(&id)?;
        let n = &nodes[idx];
        match FLOW_DIR {
            FlowDir::LeftToRight => {
                let y = n.y + 1;
                let x = n.x.saturating_sub(1);
                Some(CachedArrow {
                    x,
                    y,
                    ch: '>',
                    color_kind: COLOR_ACCENT,
                })
            }
            FlowDir::TopToBottom => {
                let x_center = n.x + n.w / 2;
                let x = x_center.min(width.saturating_sub(1));
                let y = n.y.saturating_sub(1).min(height.saturating_sub(1));
                Some(CachedArrow {
                    x,
                    y,
                    ch: 'v',
                    color_kind: COLOR_ACCENT,
                })
            }
        }
    });

    // Label the entry point so the initial state is unambiguous.
    if let Some(a) = start_arrow {
        let text = "[start]";
        let chip_w = text.chars().count() as u16;
        if chip_w + 2 < width {
            let mut x = a.x.saturating_add(2);
            if x + chip_w > width {
                x = a.x.saturating_sub(chip_w.saturating_add(2));
            }
            x = x.min(width.saturating_sub(chip_w));
            labels_out.push(CachedLabel {
                x,
                y: a.y,
                text: text.to_string(),
                color_kind: COLOR_ACCENT,
            });
        }
    }
    if let Some(a) = start_arrow {
        match FLOW_DIR {
            FlowDir::LeftToRight => {
                // Wire stub from the left edge to the start arrow.
                if a.y < height {
                    for x in 0..=a.x.min(width.saturating_sub(1)) {
                        let idx = grid_idx(width, x, a.y);
                        wire_used[idx] = wire_used[idx].saturating_add(1);
                        if wire_color[idx] == 0 {
                            wire_color[idx] = COLOR_ACCENT;
                        } else if wire_color[idx] != COLOR_ACCENT {
                            wire_color[idx] = COLOR_JUNCTION;
                        }
                    }
                    for x in 0..a.x.min(width.saturating_sub(1)) {
                        let a_idx = grid_idx(width, x, a.y);
                        let b_idx = grid_idx(width, x + 1, a.y);
                        wire_mask[a_idx] |= BIT_R;
                        wire_mask[b_idx] |= BIT_L;
                    }
                }
            }
            FlowDir::TopToBottom => {
                // Wire stub from the top edge to the start arrow.
                if a.x < width {
                    for y in 0..=a.y.min(height.saturating_sub(1)) {
                        let idx = grid_idx(width, a.x, y);
                        wire_used[idx] = wire_used[idx].saturating_add(1);
                        if wire_color[idx] == 0 {
                            wire_color[idx] = COLOR_ACCENT;
                        } else if wire_color[idx] != COLOR_ACCENT {
                            wire_color[idx] = COLOR_JUNCTION;
                        }
                    }
                    for y in 0..a.y.min(height.saturating_sub(1)) {
                        let a_idx = grid_idx(width, a.x, y);
                        let b_idx = grid_idx(width, a.x, y + 1);
                        wire_mask[a_idx] |= BIT_D;
                        wire_mask[b_idx] |= BIT_U;
                    }
                }
            }
        }
    }

    CachedCircuitGraph {
        width,
        height,
        message: None,
        nodes,
        wire_mask,
        wire_color,
        arrows,
        labels: labels_out,
        start_arrow,
    }
}

fn compute_node_width(graph: &GraphSpec) -> u16 {
    let max_label = graph
        .nodes
        .iter()
        .map(|n| n.label.chars().count())
        .max()
        .unwrap_or(1) as u16;
    // border + padding on each side.
    let inner = max_label + NODE_PAD_X * 2;
    (inner + 2).max(NODE_MIN_W)
}

fn compute_gap(total: u16, count: u16, item: u16, margin: u16) -> u16 {
    if count <= 1 {
        return 0;
    }
    let usable = total.saturating_sub(margin * 2);
    let needed_items = count.saturating_mul(item);
    if usable <= needed_items {
        return 0;
    }
    let remaining = usable - needed_items;
    remaining / (count - 1)
}

fn centered_start(total: u16, count: u16, item: u16, margin: u16, gap: u16) -> u16 {
    let count = count.max(1);
    let usable = total.saturating_sub(margin * 2);
    let items = count.saturating_mul(item);
    let gaps = count.saturating_sub(1).saturating_mul(gap);
    let grid = items.saturating_add(gaps);
    let extra = usable.saturating_sub(grid);
    margin + extra / 2
}

fn assign_layers(graph: &GraphSpec) -> HashMap<usize, usize> {
    let mut layers: HashMap<usize, usize> = HashMap::new();

    let start = graph
        .start
        .or_else(|| graph.nodes.iter().map(|n| n.id).min());

    let Some(start_id) = start else {
        return layers;
    };

    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
    for e in &graph.edges {
        adj.entry(e.from).or_default().push(e.to);
    }
    for vs in adj.values_mut() {
        vs.sort_unstable();
        vs.dedup();
    }

    let mut q: VecDeque<usize> = VecDeque::new();
    layers.insert(start_id, 0);
    q.push_back(start_id);

    while let Some(u) = q.pop_front() {
        let du = layers.get(&u).copied().unwrap_or(0);
        let Some(vs) = adj.get(&u) else {
            continue;
        };
        for &v in vs {
            if layers.contains_key(&v) {
                continue;
            }
            layers.insert(v, du + 1);
            q.push_back(v);
        }
    }

    // Unreachable nodes: place them in the last layer so they still show.
    let max_layer = layers.values().copied().max().unwrap_or(0);
    let unreachable_layer = max_layer.saturating_add(1);
    for n in &graph.nodes {
        if !layers.contains_key(&n.id) {
            layers.insert(n.id, unreachable_layer);
        }
    }

    layers
}

fn port_offset(idx: usize) -> i16 {
    if idx == 0 {
        return 0;
    }
    let k = ((idx + 1) / 2) as i16;
    if idx % 2 == 1 {
        -k
    } else {
        k
    }
}

fn anchor_right(node: &CachedNode, port: i16, width: u16, height: u16) -> (u16, u16) {
    let x = (node.x + node.w).min(width.saturating_sub(1));
    let y0 = node.y as i16 + 1 + port;
    let y = y0.clamp(0, height.saturating_sub(1) as i16) as u16;
    (x, y)
}

fn anchor_left(node: &CachedNode, port: i16, width: u16, height: u16) -> (u16, u16) {
    let x = node.x.saturating_sub(1).min(width.saturating_sub(1));
    let y0 = node.y as i16 + 1 + port;
    let y = y0.clamp(0, height.saturating_sub(1) as i16) as u16;
    (x, y)
}

fn anchor_top(node: &CachedNode, port: i16, width: u16, height: u16) -> (u16, u16) {
    let y = node.y.saturating_sub(1).min(height.saturating_sub(1));
    let x_center = node.x as i16 + (node.w as i16) / 2;
    let x0 = x_center + port;
    let x = x0.clamp(0, width.saturating_sub(1) as i16) as u16;
    (x, y)
}

fn anchor_bottom(node: &CachedNode, port: i16, width: u16, height: u16) -> (u16, u16) {
    let y = (node.y + NODE_H).min(height.saturating_sub(1));
    let x_center = node.x as i16 + (node.w as i16) / 2;
    let x0 = x_center + port;
    let x = x0.clamp(0, width.saturating_sub(1) as i16) as u16;
    (x, y)
}

fn chip_anchor_x(mid_x: u16, width: u16, chip_w: u16) -> u16 {
    if chip_w >= width {
        return 0;
    }
    let half = (chip_w / 2) as i16;
    let x0 = mid_x as i16 - half;
    x0.clamp(0, (width - chip_w) as i16) as u16
}

fn route_path(
    start: (u16, u16),
    end: (u16, u16),
    width: u16,
    height: u16,
    blocked: &[bool],
    wire_used: &[u8],
    flow: FlowDir,
) -> Option<Vec<(u16, u16)>> {
    if start == end {
        return Some(vec![start]);
    }
    let w = width as i32;
    let h = height as i32;

    let start_idx = grid_idx(width, start.0, start.1);
    let end_idx = grid_idx(width, end.0, end.1);

    let mut g_score = vec![u32::MAX; blocked.len()];
    let mut prev = vec![usize::MAX; blocked.len()];

    let (sx, sy) = (start.0 as i32, start.1 as i32);
    let (ex, ey) = (end.0 as i32, end.1 as i32);
    let dx = ex - sx;
    let dy = ey - sy;
    let dirs = preferred_dirs(dx, dy, flow);

    let mut heap: std::collections::BinaryHeap<(
        Reverse<u32>,
        Reverse<u32>,
        Reverse<u16>,
        Reverse<u16>,
        usize,
    )> = std::collections::BinaryHeap::new();

    g_score[start_idx] = 0;
    heap.push((
        Reverse(manhattan(sx, sy, ex, ey)),
        Reverse(0),
        Reverse(start.1),
        Reverse(start.0),
        start_idx,
    ));

    while let Some((_, Reverse(g), _, _, cur)) = heap.pop() {
        if cur == end_idx {
            break;
        }
        if g != g_score[cur] {
            continue;
        }

        let (cx, cy) = idx_xy(width, cur);
        for (nx, ny) in dirs
            .iter()
            .filter_map(|d| step(cx as i32, cy as i32, *d, w, h))
        {
            let nxu = nx as u16;
            let nyu = ny as u16;
            let ni = grid_idx(width, nxu, nyu);
            if ni != end_idx && blocked[ni] {
                continue;
            }
            let mut step_cost = COST_STEP;
            let used = wire_used[ni] as u32;
            if used > 0 {
                step_cost = step_cost.saturating_add(COST_WIRE_OVERLAP * used);
            }
            let g2 = g.saturating_add(step_cost);
            if g2 < g_score[ni] || (g2 == g_score[ni] && cur < prev[ni]) {
                g_score[ni] = g2;
                prev[ni] = cur;
                let h2 = manhattan(nx, ny, ex, ey);
                let f2 = g2.saturating_add(h2);
                heap.push((Reverse(f2), Reverse(g2), Reverse(nyu), Reverse(nxu), ni));
            }
        }
    }

    if prev[end_idx] == usize::MAX {
        return None;
    }

    let mut path_rev: Vec<(u16, u16)> = Vec::new();
    let mut cur = end_idx;
    loop {
        let (x, y) = idx_xy(width, cur);
        path_rev.push((x, y));
        if cur == start_idx {
            break;
        }
        cur = prev[cur];
        if cur == usize::MAX {
            return None;
        }
    }
    path_rev.reverse();
    Some(path_rev)
}

#[derive(Copy, Clone)]
enum Dir {
    Up,
    Right,
    Down,
    Left,
}

fn preferred_dirs(dx: i32, dy: i32, flow: FlowDir) -> [Dir; 4] {
    let horiz = if dx >= 0 { Dir::Right } else { Dir::Left };
    let horiz_other = if dx >= 0 { Dir::Left } else { Dir::Right };
    let vert = if dy >= 0 { Dir::Down } else { Dir::Up };
    let vert_other = if dy >= 0 { Dir::Up } else { Dir::Down };
    match flow {
        FlowDir::LeftToRight => [horiz, vert, horiz_other, vert_other],
        FlowDir::TopToBottom => [vert, horiz, vert_other, horiz_other],
    }
}

fn step(x: i32, y: i32, dir: Dir, w: i32, h: i32) -> Option<(i32, i32)> {
    let (nx, ny) = match dir {
        Dir::Up => (x, y - 1),
        Dir::Right => (x + 1, y),
        Dir::Down => (x, y + 1),
        Dir::Left => (x - 1, y),
    };
    if nx < 0 || ny < 0 || nx >= w || ny >= h {
        return None;
    }
    Some((nx, ny))
}

fn manhattan(x: i32, y: i32, ex: i32, ey: i32) -> u32 {
    ((ex - x).abs() + (ey - y).abs()) as u32 * COST_STEP
}

fn apply_wire_path(
    path: &[(u16, u16)],
    width: u16,
    wire_mask: &mut [u8],
    wire_color: &mut [u8],
    wire_used: &mut [u8],
    color_kind: u8,
) {
    // Mark cells and connectivity.
    for &p in path {
        let idx = grid_idx(width, p.0, p.1);
        wire_used[idx] = wire_used[idx].saturating_add(1);
        if wire_color[idx] == 0 {
            wire_color[idx] = color_kind;
        } else if wire_color[idx] != color_kind {
            wire_color[idx] = COLOR_JUNCTION;
        }
    }

    for w in path.windows(2) {
        let a = w[0];
        let b = w[1];
        let ai = grid_idx(width, a.0, a.1);
        let bi = grid_idx(width, b.0, b.1);
        if b.0 > a.0 {
            wire_mask[ai] |= BIT_R;
            wire_mask[bi] |= BIT_L;
        } else if b.0 < a.0 {
            wire_mask[ai] |= BIT_L;
            wire_mask[bi] |= BIT_R;
        } else if b.1 > a.1 {
            wire_mask[ai] |= BIT_D;
            wire_mask[bi] |= BIT_U;
        } else if b.1 < a.1 {
            wire_mask[ai] |= BIT_U;
            wire_mask[bi] |= BIT_D;
        }
    }
}

fn arrow_char(a: (u16, u16), b: (u16, u16)) -> char {
    if b.0 > a.0 {
        '>'
    } else if b.0 < a.0 {
        '<'
    } else if b.1 > a.1 {
        'v'
    } else {
        '^'
    }
}

const BIT_U: u8 = 1 << 0;
const BIT_R: u8 = 1 << 1;
const BIT_D: u8 = 1 << 2;
const BIT_L: u8 = 1 << 3;

fn wire_char_from_mask(mask: u8) -> char {
    let horiz = (mask & (BIT_L | BIT_R)) != 0;
    let vert = (mask & (BIT_U | BIT_D)) != 0;
    if horiz && vert {
        '+'
    } else if horiz {
        '-'
    } else {
        '|'
    }
}

const COLOR_NONE: u8 = 0;
const COLOR_WIRE0: u8 = 1;
const COLOR_WIRE1: u8 = 2;
const COLOR_WIRE2: u8 = 3;
const COLOR_WIRE3: u8 = 4;
const COLOR_JUNCTION: u8 = 5;
const COLOR_ACCENT: u8 = 6;

fn edge_color_kind(label: &str) -> u8 {
    let s = label.trim();
    let Some(c) = s.chars().next() else {
        return COLOR_WIRE0;
    };
    if c.is_ascii_digit() {
        let idx = (c as u8).saturating_sub(b'0') as usize;
        return match idx % 4 {
            0 => COLOR_WIRE0,
            1 => COLOR_WIRE1,
            2 => COLOR_WIRE2,
            _ => COLOR_WIRE3,
        };
    }
    match c.to_ascii_lowercase() {
        'p' => COLOR_WIRE1,
        _ => COLOR_WIRE0,
    }
}

fn wire_color_from_kind(kind: u8, theme: &Theme) -> Color {
    match kind {
        COLOR_WIRE0 => theme.title,
        COLOR_WIRE1 => theme.accent,
        COLOR_WIRE2 => theme.warning,
        COLOR_WIRE3 => theme.border_focused,
        COLOR_JUNCTION => theme.border,
        COLOR_ACCENT => theme.accent,
        COLOR_NONE | _ => theme.border_focused,
    }
}

fn draw_node_box(
    buf: &mut Buffer,
    area: Rect,
    node: &CachedNode,
    border: Color,
    fill: Color,
    theme: &Theme,
) {
    let w = node.w;
    let h = NODE_H;
    if node.x + w > area.width || node.y + h > area.height {
        return;
    }

    let style_border = Style::default().fg(border).bg(theme.background);
    let style_fill = Style::default().bg(fill);

    // Top / bottom.
    for dx in 0..w {
        let ch = if dx == 0 || dx == w - 1 { '+' } else { '-' };
        buf.get_mut(area.x + node.x + dx, area.y + node.y)
            .set_char(ch)
            .set_style(style_border);
        buf.get_mut(area.x + node.x + dx, area.y + node.y + (h - 1))
            .set_char(ch)
            .set_style(style_border);
    }

    if node.is_start && w >= 5 {
        let cx = node.x + w / 2;
        buf.get_mut(area.x + cx, area.y + node.y)
            .set_char('S')
            .set_style(
                Style::default()
                    .fg(theme.accent)
                    .bg(theme.background)
                    .add_modifier(Modifier::BOLD),
            );
    }

    // Middle row.
    for dx in 0..w {
        let ch = if dx == 0 || dx == w - 1 { '|' } else { ' ' };
        buf.get_mut(area.x + node.x + dx, area.y + node.y + 1)
            .set_char(ch)
            .set_style(if ch == ' ' { style_fill } else { style_border });
    }

    // Label centered.
    let label = node.label.as_str();
    let label_w = label.chars().count() as u16;
    if label_w > 0 {
        let inner_w = w.saturating_sub(2);
        let start_x = node.x + 1 + inner_w.saturating_sub(label_w) / 2;
        let style = Style::default()
            .fg(theme.foreground)
            .bg(fill)
            .add_modifier(Modifier::BOLD);
        buf.set_stringn(
            area.x + start_x,
            area.y + node.y + 1,
            label,
            label_w as usize,
            style,
        );
    }
}

fn grid_idx(width: u16, x: u16, y: u16) -> usize {
    (y as usize) * (width as usize) + (x as usize)
}

fn idx_xy(width: u16, idx: usize) -> (u16, u16) {
    let w = width as usize;
    let y = idx / w;
    let x = idx % w;
    (x as u16, y as u16)
}

// --- Stable hashing (cache key seed) ---

struct StableHasher {
    state: u64,
}

impl StableHasher {
    fn new() -> Self {
        Self {
            state: 0xcbf29ce484222325,
        }
    }

    fn write_u64(&mut self, v: u64) {
        self.write_bytes(&v.to_le_bytes());
    }

    fn write_str(&mut self, s: &str) {
        self.write_u64(s.len() as u64);
        self.write_bytes(s.as_bytes());
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        const PRIME: u64 = 0x100000001b3;
        for &b in bytes {
            self.state ^= b as u64;
            self.state = self.state.wrapping_mul(PRIME);
        }
    }

    fn finish(self) -> u64 {
        self.state
    }
}

fn graph_hash(graph: &GraphSpec) -> u64 {
    let mut h = StableHasher::new();
    h.write_u64(2); // format version
    h.write_u64(graph.nodes.len() as u64);
    h.write_u64(graph.edges.len() as u64);
    h.write_u64(graph.start.unwrap_or(usize::MAX) as u64);

    let mut nodes: Vec<&GraphNode> = graph.nodes.iter().collect();
    nodes.sort_by_key(|n| n.id);
    for n in nodes {
        h.write_u64(n.id as u64);
        h.write_str(&n.label);
        h.write_u64(n.output.map(|c| c as u64).unwrap_or(u64::MAX));
    }

    let mut edges: Vec<&GraphEdge> = graph.edges.iter().collect();
    edges.sort_by(|a, b| {
        a.from
            .cmp(&b.from)
            .then_with(|| a.to.cmp(&b.to))
            .then_with(|| a.label.cmp(&b.label))
    });
    for e in edges {
        h.write_u64(e.from as u64);
        h.write_u64(e.to as u64);
        h.write_str(&e.label);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_offset_pattern() {
        assert_eq!(port_offset(0), 0);
        assert_eq!(port_offset(1), -1);
        assert_eq!(port_offset(2), 1);
        assert_eq!(port_offset(3), -2);
        assert_eq!(port_offset(4), 2);
    }

    #[test]
    fn wire_char_basic() {
        assert_eq!(wire_char_from_mask(BIT_L | BIT_R), '-');
        assert_eq!(wire_char_from_mask(BIT_U | BIT_D), '|');
        assert_eq!(wire_char_from_mask(BIT_U | BIT_R), '+');
    }

    #[test]
    fn layers_are_deterministic() {
        let g = GraphSpec {
            nodes: vec![
                GraphNode {
                    id: 0,
                    label: "0".into(),
                    output: None,
                },
                GraphNode {
                    id: 1,
                    label: "1".into(),
                    output: None,
                },
                GraphNode {
                    id: 2,
                    label: "2".into(),
                    output: None,
                },
            ],
            edges: vec![
                GraphEdge {
                    from: 0,
                    to: 1,
                    label: "a".into(),
                },
                GraphEdge {
                    from: 1,
                    to: 2,
                    label: "b".into(),
                },
            ],
            start: Some(0),
        };
        let a = assign_layers(&g);
        let b = assign_layers(&g);
        assert_eq!(a, b);
        assert_eq!(a.get(&0), Some(&0));
        assert_eq!(a.get(&1), Some(&1));
        assert_eq!(a.get(&2), Some(&2));
    }

    #[test]
    fn route_path_prefers_straight_line() {
        let width = 20;
        let height = 7;
        let blocked = vec![false; (width as usize) * (height as usize)];
        let used = vec![0u8; (width as usize) * (height as usize)];
        let path = route_path(
            (0, 3),
            (19, 3),
            width,
            height,
            &blocked,
            &used,
            FlowDir::LeftToRight,
        )
        .unwrap();
        assert_eq!(path.first().copied(), Some((0, 3)));
        assert_eq!(path.last().copied(), Some((19, 3)));
        // Expect a direct run.
        assert_eq!(path.len() as u16, 20);
    }
}
