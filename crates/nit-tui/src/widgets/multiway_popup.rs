//! Phase 6b — live multiway-search tree popup.
//!
//! A toggleable overlay alongside `artifacts_popup` / `gate_monitor_view` that
//! renders the *whole* best-first search as a depth-indented tree (v1's DAG is a
//! tree; Phase-4 merges show as multi-parent refs). Each node is one row,
//! coloured red→green by value with a status glyph, its score/tier, and its turn
//! summary; the best/kept path is gold and the frontier pick the engine will
//! expand next carries a spinner. A header shows mood·k, turns/nodes vs budget,
//! best score/tier, and the live stop reason.
//!
//! This is pure rendering over [`nit_core::MultiwayView`]: the widget owns no
//! state and reads the live model passed in, so closing and reopening it is
//! non-destructive. Streaming the model, the toggle flag, and the scroll offset
//! are wired by the runtime / state layer, not here.

use std::collections::HashSet;

use nit_core::{AgentsState, MultiwayHeader, MultiwayNodeStatus, MultiwayNodeView, MultiwayView};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};

use crate::theme::Theme;

const TITLE: &str = " MULTIWAY SEARCH ";
const SPINNER: &str = "⟳";
/// Subtrees deeper than this collapse into one `⋯ N collapsed` marker so a
/// runaway DAG can't blow the row budget; shallower trees render in full.
const COLLAPSE_DEPTH: u16 = 6;
const IDLE_HINT: &str = "no multiway search active";

/// A centred, content-appropriate popup rect over `screen` — the overlay is not
/// full-screen, matching the other `widgets/` popups.
pub fn preferred_size(screen: Rect) -> Rect {
    let width = (screen.width.saturating_mul(80) / 100)
        .clamp(60, 160)
        .min(screen.width.saturating_sub(2));
    let height = (screen.height.saturating_mul(70) / 100)
        .clamp(12, 44)
        .min(screen.height.saturating_sub(2));
    let x = screen.x + screen.width.saturating_sub(width) / 2;
    let y = screen.y + screen.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// Frozen seam (`docs/MULTIWAY.md` Phase 6b): int-keys gates on
/// `show_multiway_popup` and calls this with `&state.agents`. The popup reads the
/// live [`MultiwayView`] and scroll offset out of state and owns nothing, so
/// close/reopen is non-destructive. Before the first snapshot streams it draws an
/// idle hint rather than nothing — its key handler is modal (swallows scroll/close
/// keys), so painting nothing would make toggling it open an invisible key-trap.
pub fn render(frame: &mut Frame<'_>, area: Rect, agents: &AgentsState, theme: &Theme) {
    match agents.multiway_view.as_ref() {
        Some(view) => render_view(frame, area, view, agents.multiway_popup_scroll, theme),
        None => render_idle(frame, area, theme),
    }
}

/// The shared bordered, titled popup frame, built once so the live and idle paths
/// can't drift in border or title styling.
fn popup_block(theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_focused))
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .title(Line::from(Span::styled(
            TITLE,
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        )))
}

/// Toggled open before any search has streamed: paint the chrome and an idle hint
/// so the (modal, key-swallowing) popup is visible rather than a silent key-trap.
fn render_idle(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    frame.render_widget(Clear, area);
    let block = popup_block(theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let hint = Paragraph::new(Line::from(Span::styled(
        IDLE_HINT.to_string(),
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::DIM | Modifier::ITALIC),
    )))
    .style(Style::default().bg(theme.background).fg(theme.foreground));
    frame.render_widget(hint, inner);
}

/// Draw the popup chrome, pinned header, and scrolling tree for `view`. `scroll`
/// is clamped here so an over-scroll can't blank the body.
fn render_view(frame: &mut Frame<'_>, area: Rect, view: &MultiwayView, scroll: u16, theme: &Theme) {
    frame.render_widget(Clear, area);
    let block = popup_block(theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let body_bg = Style::default().bg(theme.background).fg(theme.foreground);
    let header = header_lines(&view.header, theme);
    let header_h = (header.len() as u16).min(inner.height);
    frame.render_widget(
        Paragraph::new(header).style(body_bg),
        Rect {
            height: header_h,
            ..inner
        },
    );

    let body_h = inner.height.saturating_sub(header_h).saturating_sub(1);
    if body_h == 0 {
        return;
    }
    let rows = tree_rows(view, theme, inner.width);
    let scroll = scroll.min((rows.len() as u16).saturating_sub(body_h));
    frame.render_widget(
        Paragraph::new(rows).style(body_bg).scroll((scroll, 0)),
        Rect {
            y: inner.y + header_h + 1,
            height: body_h,
            ..inner
        },
    );
}

fn header_lines(header: &MultiwayHeader, theme: &Theme) -> Vec<Line<'static>> {
    let dim = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let strong = Style::default()
        .fg(theme.title_focused)
        .add_modifier(Modifier::BOLD);
    let value = Style::default().fg(theme.foreground);

    let line1 = Line::from(vec![
        Span::styled(format!("{}·k{}", header.mood, header.k), strong),
        Span::styled("   turns ", dim),
        Span::styled(format!("{}/{}", header.turns, header.max_turns), value),
        Span::styled("   nodes ", dim),
        Span::styled(format!("{}/{}", header.nodes, header.max_nodes), value),
        Span::styled("   frontier ", dim),
        Span::styled(header.frontier.to_string(), value),
    ]);

    let best = Style::default()
        .fg(value_color(header.best_score, theme))
        .add_modifier(Modifier::BOLD);
    let stop = header
        .stop_reason
        .clone()
        .unwrap_or_else(|| "searching…".to_string());
    let line2 = Line::from(vec![
        Span::styled("best ", dim),
        Span::styled(
            format!("{:.2} {}", header.best_score, header.best_tier.numeral()),
            best,
        ),
        Span::styled("   ", dim),
        Span::styled(
            stop,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::ITALIC),
        ),
    ]);
    vec![line1, line2]
}

/// Flatten the pre-order node list into rows, collapsing any subtree past
/// [`COLLAPSE_DEPTH`] into a single marker. The list is pre-order, so a subtree's
/// deep descendants are the contiguous run with `depth > COLLAPSE_DEPTH`.
fn tree_rows(view: &MultiwayView, theme: &Theme, width: u16) -> Vec<Line<'static>> {
    if view.nodes.is_empty() {
        return vec![Line::from(Span::styled(
            IDLE_HINT.to_string(),
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::DIM | Modifier::ITALIC),
        ))];
    }
    let kept: HashSet<&str> = view.kept_path.iter().map(String::as_str).collect();
    let in_flight = view.in_flight.as_deref();
    let mut rows = Vec::with_capacity(view.nodes.len());
    let mut i = 0;
    while i < view.nodes.len() {
        if view.nodes[i].depth > COLLAPSE_DEPTH {
            let start = i;
            while i < view.nodes.len() && view.nodes[i].depth > COLLAPSE_DEPTH {
                i += 1;
            }
            rows.push(collapse_marker(COLLAPSE_DEPTH + 1, i - start, theme));
        } else {
            rows.push(node_line(&view.nodes[i], &kept, in_flight, theme, width));
            i += 1;
        }
    }
    rows
}

fn node_line(
    node: &MultiwayNodeView,
    kept: &HashSet<&str>,
    in_flight: Option<&str>,
    theme: &Theme,
    width: u16,
) -> Line<'static> {
    let is_kept = kept.contains(node.id.as_str()) || kept.contains(node.short_id.as_str());
    let is_next = in_flight == Some(node.id.as_str()) || in_flight == Some(node.short_id.as_str());

    let (glyph, glyph_color) = if is_next {
        (SPINNER, theme.accent)
    } else {
        (status_glyph(node.status), status_color(node.status, theme))
    };
    let score_color = node.score.map_or(theme.border, |s| value_color(s, theme));
    let budget = (width as usize)
        .saturating_sub(node.depth as usize * 2 + 28)
        .max(8);

    let mut spans = vec![
        Span::raw("  ".repeat(node.depth as usize)),
        Span::styled(format!("{glyph} "), Style::default().fg(glyph_color)),
        Span::styled(
            format!("{} ", node.short_id),
            Style::default().fg(score_color),
        ),
        Span::styled(
            score_label(node),
            Style::default()
                .fg(score_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", first_line(&node.summary, budget)),
            Style::default().fg(theme.foreground),
        ),
    ];
    push_suffix(&mut spans, node, theme, is_next);

    if is_kept {
        let gold = Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD);
        for span in spans.iter_mut() {
            if !span.content.trim().is_empty() {
                span.style = gold;
            }
        }
    }
    Line::from(spans)
}

/// Trailing annotations: changed-file count, merge fan-in for multi-parent
/// nodes, and a `‹next›` tag on the frontier pick that carries the spinner.
fn push_suffix(
    spans: &mut Vec<Span<'static>>,
    node: &MultiwayNodeView,
    theme: &Theme,
    is_next: bool,
) {
    if node.changed > 0 {
        spans.push(Span::styled(
            format!(" ({} files)", node.changed),
            Style::default().fg(theme.title).add_modifier(Modifier::DIM),
        ));
    }
    if node.parents.len() > 1 {
        spans.push(Span::styled(
            format!(" ⇄{}", node.parents.len()),
            Style::default().fg(theme.accent),
        ));
    }
    if is_next {
        spans.push(Span::styled(
            " ‹next›".to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::DIM),
        ));
    }
}

fn collapse_marker(depth: u16, hidden: usize, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::raw("  ".repeat(depth as usize)),
        Span::styled(
            format!("⋯ {hidden} collapsed"),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM | Modifier::ITALIC),
        ),
    ])
}

fn score_label(node: &MultiwayNodeView) -> String {
    match (node.score, node.tier) {
        (Some(score), Some(tier)) => format!("{score:.2} {}", tier.numeral()),
        (Some(score), None) => format!("{score:.2}"),
        (None, _) => "·".to_string(),
    }
}

/// Status glyphs match the spec in `docs/MULTIWAY.md` (Phase 6b).
fn status_glyph(status: MultiwayNodeStatus) -> &'static str {
    match status {
        MultiwayNodeStatus::Open => "●",
        MultiwayNodeStatus::Held => "⊙",
        MultiwayNodeStatus::Expanded => "○",
        MultiwayNodeStatus::GateFailed => "✗",
        MultiwayNodeStatus::Solution => "★",
        MultiwayNodeStatus::Dominated => "◌",
    }
}

fn status_color(status: MultiwayNodeStatus, theme: &Theme) -> Color {
    match status {
        MultiwayNodeStatus::Open => theme.border_focused,
        MultiwayNodeStatus::Held => theme.warning,
        MultiwayNodeStatus::Expanded => theme.title,
        MultiwayNodeStatus::GateFailed => theme.error,
        MultiwayNodeStatus::Solution => theme.accent,
        MultiwayNodeStatus::Dominated => theme.border,
    }
}

/// Map a value in `0.0..=1.0` to a red→amber→green ramp through the theme's
/// error/warning/success colours, so the gradient tracks the operator's palette.
fn value_color(score: f32, theme: &Theme) -> Color {
    let s = score.clamp(0.0, 1.0);
    if s < 0.5 {
        lerp_color(theme.error, theme.warning, s * 2.0)
    } else {
        lerp_color(theme.warning, theme.success, (s - 0.5) * 2.0)
    }
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let (ar, ag, ab) = rgb(a);
    let (br, bg, bb) = rgb(b);
    Color::Rgb(
        lerp_channel(ar, br, t),
        lerp_channel(ag, bg, t),
        lerp_channel(ab, bb, t),
    )
}

fn lerp_channel(a: u8, b: u8, t: f32) -> u8 {
    let t = t.clamp(0.0, 1.0);
    (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8
}

fn rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (200, 200, 200),
    }
}

/// First line of an (untrusted, possibly multi-line) agent summary, trimmed and
/// truncated to `max` display chars with an ellipsis. Control characters collapse
/// to a space (mirroring `Graph::to_dot`'s `dot_escape`) so raw ANSI / `\t` / `\r`
/// in agent text can't inject escape sequences into the rendered `Span`.
fn first_line(summary: &str, max: usize) -> String {
    let sanitized: String = summary
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = sanitized.trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let mut out: String = line.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
#[path = "tests/multiway_popup.rs"]
mod tests;
