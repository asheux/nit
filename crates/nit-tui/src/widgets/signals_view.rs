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

/// Render the Substrate Signals body into `inner`, caching max_scroll so the
/// scroll handlers can skip a rebuild on every wheel tick — same pattern as
/// `gate_monitor_view::render`.
pub fn render_body(frame: &mut Frame<'_>, inner: Rect, state: &mut AppState, theme: &Theme) {
    let lines = build_lines(state, theme, inner.width);
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    state.substrate_last_max_scroll = max_scroll;
    let scroll = state.substrate_scroll.min(max_scroll) as u16;
    state.substrate_scroll = scroll as usize;
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
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::DIM),
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
            &truncate(&signal.posted_by, 10),
            &format_target(&signal.target),
            &format!("{age}g"),
            &truncate(&signal.id, 24),
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
    // Width-adaptive: drop ID below 70; drop AGE below 50.
    let show_id = width >= 70;
    let show_age = width >= 50;
    let mut row = format!(
        "{:>4}  {:<14} {:<10} {:<20}",
        strength,
        truncate(kind, 14),
        by,
        truncate(target, 20)
    );
    if show_age {
        row.push_str(&format!(" {age:>5}"));
    }
    if show_id {
        row.push_str(&format!("  {id}"));
    }
    row
}

fn kind_label(kind: SignalKind) -> &'static str {
    match kind {
        SignalKind::Warning => "Warning",
        SignalKind::Lead => "Lead",
        SignalKind::Deadend => "Deadend",
        SignalKind::HelpNeeded => "HelpNeeded",
        SignalKind::ClaimViolation => "ClaimViol",
        SignalKind::DoneMarker => "DoneMarker",
    }
}

fn format_target(t: &SignalTarget) -> String {
    match t {
        SignalTarget::Global => "Global".to_string(),
        SignalTarget::Agent { agent_id } => format!("agent:{}", truncate(agent_id, 13)),
        SignalTarget::File { path } => format!("file:{}", truncate(&path.to_string_lossy(), 14)),
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
    // Base color by kind; intensity modulated by strength.
    let color = match kind {
        SignalKind::Warning => theme.warning,
        SignalKind::ClaimViolation => theme.error,
        SignalKind::DoneMarker => theme.success,
        SignalKind::Lead => theme.accent,
        SignalKind::HelpNeeded => theme.title_focused,
        SignalKind::Deadend => theme.border,
    };
    let mut style = Style::default().fg(color);
    if strength >= 0.7 {
        style = style.add_modifier(Modifier::BOLD);
    } else if strength < 0.3 {
        style = style.add_modifier(Modifier::DIM);
    }
    style
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
mod tests {
    use super::*;
    use nit_core::substrate::{Signal, SignalKind, SignalTarget, SubstrateState};
    use std::path::PathBuf;

    fn mk_state_with_signals(signals: Vec<Signal>) -> AppState {
        use nit_core::buffer::Buffer;
        let root = std::env::temp_dir().join(format!(
            "nit-signals-view-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut state =
            AppState::new(root, Buffer::empty("x", None), Buffer::empty("n", None));
        let mut substrate = SubstrateState::default();
        for s in signals {
            substrate.emit_signal(s);
        }
        state.substrate = substrate;
        state
    }

    fn mk_signal(id: &str, kind: SignalKind, initial: f32, posted_at: u64) -> Signal {
        Signal {
            id: id.into(),
            kind,
            posted_by: "agent-a".into(),
            posted_at_gen: posted_at,
            target: SignalTarget::Global,
            initial_strength: initial,
            payload: serde_json::Value::Null,
        }
    }

    #[test]
    fn build_lines_empty_has_header_and_hint() {
        let state = mk_state_with_signals(vec![]);
        let theme = Theme::default();
        let lines = build_lines(&state, &theme, 100);
        // summary + blank + column header + blank + empty hint = 5 lines
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn build_lines_with_two_signals_emits_rows() {
        let signals = vec![
            mk_signal("s1", SignalKind::Warning, 0.9, 0),
            mk_signal("s2", SignalKind::Lead, 0.4, 0),
        ];
        let state = mk_state_with_signals(signals);
        let theme = Theme::default();
        let lines = build_lines(&state, &theme, 100);
        // summary + blank + column header + 2 rows = 5 lines
        assert_eq!(lines.len(), 5);
    }
}
