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
use crate::widgets::text_utils::truncate_with_ellipsis as truncate;

const AGE_COL_MIN_WIDTH: u16 = 50;
const RATIONALE_COL_MIN_WIDTH: u16 = 70;
const ID_COL_MIN_WIDTH: u16 = 90;

// Caches `max_scroll` on `state` so scroll handlers can skip a rebuild on every wheel
// tick — same pattern as `claims_view::render_body`.
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
    let active = sorted.len();
    lines.push(header_line(
        &format!("{active} active   gen {current_gen}"),
        theme,
    ));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format_row("TTL", "BY", "TARGET", "AGE", "RATIONALE", "ID", width),
        Style::default().fg(theme.title).add_modifier(Modifier::DIM),
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

    lines.extend(
        sorted
            .into_iter()
            .map(|(assumption, remaining)| row_line(assumption, remaining, current_gen, width, theme)),
    );
    lines
}

fn row_line(
    assumption: &nit_core::substrate::Assumption,
    remaining: u64,
    current_gen: u64,
    width: u16,
    theme: &Theme,
) -> Line<'static> {
    let age = current_gen.saturating_sub(assumption.posted_at_gen);
    let row = format_row(
        &format!("{remaining}g"),
        &truncate(&assumption.posted_by, 16),
        &format_target(&assumption.target),
        &format!("{age}g"),
        &truncate(&assumption.rationale, 30),
        &truncate(&assumption.id, 24),
        width,
    );
    Line::from(Span::styled(row, style_for(remaining, theme)))
}

fn header_line(summary: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        summary.to_string(),
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::BOLD),
    ))
}

// Width-adaptive: drop ID below 90; drop RATIONALE below 70; drop AGE below 50.
// Rationale is the widest diagnostic column so it goes first when space tightens.
fn format_row(
    ttl: &str,
    by: &str,
    target: &str,
    age: &str,
    rationale: &str,
    id: &str,
    width: u16,
) -> String {
    let target = truncate(target, 28);
    let mut row = format!("{ttl:>4}  {by:<16} {target:<28}");
    if width >= AGE_COL_MIN_WIDTH {
        row.push_str(&format!(" {age:>5}"));
    }
    if width >= RATIONALE_COL_MIN_WIDTH {
        let rationale = truncate(rationale, 30);
        row.push_str(&format!("  {rationale:<30}"));
    }
    if width >= ID_COL_MIN_WIDTH {
        row.push_str(&format!("  {id}"));
    }
    row
}

fn format_target(t: &AssumptionTarget) -> String {
    match t {
        AssumptionTarget::Global => "Global".to_string(),
        AssumptionTarget::File { path } => {
            format!("file:{}", truncate(&path.to_string_lossy(), 22))
        }
        AssumptionTarget::Region {
            path,
            start_line,
            end_line,
        } => format!(
            "region:{}#{}-{}",
            truncate(&path.to_string_lossy(), 16),
            start_line,
            end_line
        ),
    }
}

// Assumptions have no kind axis, so we encode closeness-to-expiry with a single
// accent color whose intensity tracks remaining TTL.
fn style_for(remaining: u64, theme: &Theme) -> Style {
    let style = Style::default().fg(theme.accent);
    if remaining >= 2 {
        style.add_modifier(Modifier::BOLD)
    } else if remaining == 0 {
        style.add_modifier(Modifier::DIM)
    } else {
        style
    }
}

#[cfg(test)]
#[path = "tests/assumptions_view.rs"]
mod tests;
