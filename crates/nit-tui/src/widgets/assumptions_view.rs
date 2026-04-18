use nit_core::substrate::AssumptionTarget;
use nit_core::AppState;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::theme::Theme;

/// Render the Substrate Assumptions body into `inner`, caching max_scroll so
/// the scroll handlers can skip a rebuild on every wheel tick — same pattern
/// as `claims_view::render_body`.
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
    let sorted = state.substrate.assumptions_sorted_by_remaining_ttl();

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Summary header.
    let summary = format!("{} active   gen {}", sorted.len(), current_gen);
    lines.push(Line::from(Span::styled(
        summary,
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    // Column header.
    let col_header = format_row("TTL", "BY", "TARGET", "AGE", "RATIONALE", "ID", width);
    lines.push(Line::from(Span::styled(
        col_header,
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::DIM),
    )));

    if sorted.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  No active assumptions. Agents haven't posted any yet.",
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        )));
        return lines;
    }

    for (assumption, remaining) in sorted {
        let age = current_gen.saturating_sub(assumption.posted_at_gen);
        let row = format_row(
            &format!("{remaining}g"),
            &truncate(&assumption.posted_by, 10),
            &format_target(&assumption.target),
            &format!("{age}g"),
            &truncate(&assumption.rationale, 30),
            &truncate(&assumption.id, 24),
            width,
        );
        let style = style_for(remaining, theme);
        lines.push(Line::from(Span::styled(row, style)));
    }

    lines
}

fn format_row(
    ttl: &str,
    by: &str,
    target: &str,
    age: &str,
    rationale: &str,
    id: &str,
    width: u16,
) -> String {
    // Width-adaptive: drop ID below 90; drop RATIONALE below 70; drop AGE
    // below 50. Rationale is the widest diagnostic column so it goes first
    // when space tightens.
    let show_id = width >= 90;
    let show_rationale = width >= 70;
    let show_age = width >= 50;
    let mut row = format!(
        "{:>4}  {:<10} {:<20}",
        ttl,
        by,
        truncate(target, 20),
    );
    if show_age {
        row.push_str(&format!(" {age:>5}"));
    }
    if show_rationale {
        row.push_str(&format!("  {:<30}", truncate(rationale, 30)));
    }
    if show_id {
        row.push_str(&format!("  {id}"));
    }
    row
}

fn format_target(t: &AssumptionTarget) -> String {
    match t {
        AssumptionTarget::Global => "Global".to_string(),
        AssumptionTarget::File { path } => {
            format!("file:{}", truncate(&path.to_string_lossy(), 14))
        }
        AssumptionTarget::Region {
            path,
            start_line,
            end_line,
        } => format!(
            "region:{}#{}-{}",
            truncate(&path.to_string_lossy(), 10),
            start_line,
            end_line
        ),
    }
}

fn style_for(remaining: u64, theme: &Theme) -> Style {
    // Single color (accent), intensity modulated by remaining TTL — there is
    // no kind axis for assumptions, so the only dimension to encode is how
    // close we are to expiry.
    let mut style = Style::default().fg(theme.accent);
    if remaining >= 2 {
        style = style.add_modifier(Modifier::BOLD);
    } else if remaining == 0 {
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
    use nit_core::substrate::{Assumption, AssumptionTarget, SubstrateState};
    use std::path::PathBuf;

    fn mk_state_with_assumptions(assumptions: Vec<Assumption>) -> AppState {
        use nit_core::buffer::Buffer;
        let root = std::env::temp_dir().join(format!(
            "nit-assumptions-view-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut state =
            AppState::new(root, Buffer::empty("x", None), Buffer::empty("n", None));
        let mut substrate = SubstrateState::default();
        for a in assumptions {
            substrate.assumptions.insert(a.id.clone(), a);
        }
        state.substrate = substrate;
        state
    }

    fn mk_assumption(id: &str, gen: u64, ttl: u64) -> Assumption {
        Assumption {
            id: id.into(),
            target: AssumptionTarget::File {
                path: PathBuf::from("a.rs"),
            },
            fact: serde_json::Value::Null,
            posted_by: "agent-a".into(),
            posted_at_gen: gen,
            ttl_gens: ttl,
            rationale: "test".into(),
        }
    }

    #[test]
    fn build_lines_empty_has_header_and_hint() {
        let state = mk_state_with_assumptions(vec![]);
        let theme = Theme::default();
        let lines = build_lines(&state, &theme, 100);
        // summary + blank + column header + blank + empty hint = 5 lines
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn build_lines_with_two_assumptions_emits_rows() {
        let assumptions = vec![mk_assumption("a1", 0, 5), mk_assumption("a2", 0, 3)];
        let state = mk_state_with_assumptions(assumptions);
        let theme = Theme::default();
        let lines = build_lines(&state, &theme, 100);
        // summary + blank + column header + 2 rows = 5 lines
        assert_eq!(lines.len(), 5);
    }
}
