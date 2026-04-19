use nit_core::substrate::{Signal, SignalKind, SignalTarget};
use nit_core::AppState;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::theme::Theme;

// Width breakpoints for the row formatter — drop ID below WIDTH_SHOW_ID,
// drop AGE below WIDTH_SHOW_AGE. Keeping the thresholds named lets the
// column layout in `format_row` track the header in `build_lines`.
const WIDTH_SHOW_ID: u16 = 90;
const WIDTH_SHOW_AGE: u16 = 70;
const COL_KIND_W: usize = 14;
const COL_BY_W: usize = 26;
const COL_TARGET_W: usize = 36;
const STRONG_BOLD: f32 = 0.7;
const STRONG_DIM: f32 = 0.3;

/// Render the Substrate Signals body into `inner`, caching max_scroll so the
/// scroll handlers can skip a rebuild on every wheel tick — same pattern as
/// `gate_monitor_view::render`.
pub fn render_body(frame: &mut Frame<'_>, inner: Rect, state: &mut AppState, theme: &Theme) {
    let lines = build_lines(state, theme, inner.width);
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    state.substrate_overlay_last_max_scroll = max_scroll;
    let scroll = state.substrate_overlay_scroll.min(max_scroll) as u16;
    state.substrate_overlay_scroll = scroll as usize;
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(theme.background).fg(theme.foreground))
            .scroll((scroll, 0)),
        inner,
    );
}

pub fn build_lines(state: &AppState, theme: &Theme, width: u16) -> Vec<Line<'static>> {
    let current_gen = state.substrate.generation;
    let sorted = state.substrate.signals_sorted_by_strength();

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Summary header.
    let counts = count_by_kind(&sorted);
    let summary = if counts.is_empty() {
        format!("{} active   gen {}", sorted.len(), current_gen)
    } else {
        format!(
            "{} active   [{}]   gen {}",
            sorted.len(),
            counts,
            current_gen,
        )
    };
    lines.push(Line::from(Span::styled(
        summary,
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    // Column header.
    let col_header = format_row("STR", "KIND", "BY", "TARGET", "AGE", "ID", width);
    lines.push(Line::from(Span::styled(
        col_header,
        Style::default().fg(theme.title).add_modifier(Modifier::DIM),
    )));

    if sorted.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  No active signals yet. Run an agent turn.",
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        )));
        return lines;
    }

    for (signal, strength) in sorted {
        let age = current_gen.saturating_sub(signal.posted_at_gen);
        let row = format_row(
            &format!("{strength:.2}"),
            kind_label(signal.kind),
            &truncate(&compact_agent_id(&signal.posted_by), 26),
            &format_target(&signal.target),
            &format!("{age}g"),
            &truncate(&compact_agent_id(&signal.id), 48),
            width,
        );
        let style = style_for(signal.kind, strength, theme);
        lines.push(Line::from(Span::styled(row, style)));
    }

    lines
}

fn format_row(
    strength: &str,
    kind: &str,
    by: &str,
    target: &str,
    age: &str,
    id: &str,
    width: u16,
) -> String {
    let show_id = width >= WIDTH_SHOW_ID;
    let show_age = width >= WIDTH_SHOW_AGE;
    let kind_col = pad_right(&truncate(kind, COL_KIND_W), COL_KIND_W);
    let by_col = pad_right(by, COL_BY_W);
    let target_col = pad_right(&truncate(target, COL_TARGET_W), COL_TARGET_W);
    let mut row = format!("{strength:>4}  {kind_col} {by_col} {target_col}");
    if show_age {
        row.push_str(&format!(" {age:>5}"));
    }
    if show_id {
        row.push_str(&format!("  {id}"));
    }
    row
}

fn pad_right(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count >= width {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + width - count);
    out.push_str(text);
    for _ in count..width {
        out.push(' ');
    }
    out
}

fn kind_label(kind: SignalKind) -> &'static str {
    match kind {
        SignalKind::Warning => "Warning",
        SignalKind::Lead => "Lead",
        SignalKind::Deadend => "Deadend",
        SignalKind::HelpNeeded => "HelpNeeded",
        SignalKind::ClaimViolation => "ClaimViol",
        SignalKind::DoneMarker => "DoneMarker",
        SignalKind::InterventionEmitted => "Intervent",
    }
}

fn format_target(t: &SignalTarget) -> String {
    match t {
        SignalTarget::Global => "Global".to_string(),
        SignalTarget::Agent { agent_id } => {
            format!("agent:{}", truncate(&compact_agent_id(agent_id), 30))
        }
        SignalTarget::File { path } => {
            format!(
                "file:{}",
                truncate(&compact_path(&path.to_string_lossy()), 30)
            )
        }
    }
}

fn count_by_kind(signals: &[(&Signal, f32)]) -> String {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (s, _) in signals {
        *map.entry(kind_label(s.kind)).or_insert(0) += 1;
    }
    map.into_iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn style_for(kind: SignalKind, strength: f32, theme: &Theme) -> Style {
    let color = match kind {
        SignalKind::Warning => theme.warning,
        SignalKind::ClaimViolation | SignalKind::InterventionEmitted => theme.error,
        SignalKind::DoneMarker => theme.success,
        SignalKind::Lead => theme.accent,
        SignalKind::HelpNeeded => theme.title_focused,
        SignalKind::Deadend => theme.border,
    };
    let mut style = Style::default().fg(color);
    if strength >= STRONG_BOLD {
        style = style.add_modifier(Modifier::BOLD);
    } else if strength < STRONG_DIM {
        style = style.add_modifier(Modifier::DIM);
    }
    style
}

/// Strip absolute-path noise: if the path contains a `crates/` segment,
/// show it rooted at `crates/` (workspace-relative). Otherwise show just
/// the final filename.
fn compact_path(p: &str) -> String {
    if let Some(idx) = p.find("crates/") {
        return p[idx..].to_string();
    }
    std::path::Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string())
}

/// Strip the swarm/mission middle segment from a clone agent id so the UI
/// shows `claude-opus-4-7#clone-01` instead of
/// `claude-opus-4-7#swarm-mis-001-clone-01`. Non-clone ids pass through.
fn compact_agent_id(id: &str) -> String {
    match parse_swarm_clone_id(id) {
        Some((base, suffix)) => format!("{base}#{suffix}"),
        None => id.to_string(),
    }
}

fn parse_swarm_clone_id(id: &str) -> Option<(&str, &str)> {
    let (base, rest) = id.split_once("#swarm-")?;
    let suffix = rest.splitn(3, '-').nth(2).filter(|s| !s.is_empty())?;
    Some((base, suffix))
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('\u{2026}');
        t
    }
}

#[cfg(test)]
#[path = "tests/signals_view.rs"]
mod tests;
