use nit_core::substrate::{Claim, ClaimKind, ClaimTarget};
use nit_core::AppState;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::theme::Theme;

/// Render the Substrate Claims body into `inner`, caching max_scroll so the
/// scroll handlers can skip a rebuild on every wheel tick — same pattern as
/// `signals_view::render_body`.
pub fn render_body(frame: &mut Frame<'_>, inner: Rect, state: &mut AppState, theme: &Theme) {
    let lines = build_lines(state, theme, inner.width);
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    state.substrate_claims_last_max_scroll = max_scroll;
    let scroll = state.substrate_claims_scroll.min(max_scroll) as u16;
    state.substrate_claims_scroll = scroll as usize;
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(theme.background).fg(theme.foreground))
            .scroll((scroll, 0)),
        inner,
    );
}

pub fn build_lines(state: &AppState, theme: &Theme, width: u16) -> Vec<Line<'static>> {
    let current_gen = state.substrate.generation;
    let sorted = state.substrate.claims_sorted_by_remaining_ttl();

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
    let col_header = format_row("TTL", "KIND", "BY", "TARGET", "AGE", "ID", width);
    lines.push(Line::from(Span::styled(
        col_header,
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::DIM),
    )));

    if sorted.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  No active claims. Agents haven't written files yet.",
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        )));
        return lines;
    }

    for (claim, remaining) in sorted {
        let age = current_gen.saturating_sub(claim.claimed_at_gen);
        let row = format_row(
            &format!("{remaining}g"),
            kind_label(claim.kind),
            &truncate(&claim.claimed_by, 10),
            &format_target(&claim.target),
            &format!("{age}g"),
            &truncate(&claim.id, 24),
            width,
        );
        let style = style_for(claim.kind, remaining, theme);
        lines.push(Line::from(Span::styled(row, style)));
    }

    lines
}

fn format_row(
    ttl: &str,
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
        ttl,
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

fn kind_label(kind: ClaimKind) -> &'static str {
    match kind {
        ClaimKind::ExclusiveWrite => "ExclusiveWrite",
        ClaimKind::SharedRead => "SharedRead",
        ClaimKind::AppendOnly => "AppendOnly",
        ClaimKind::Soft => "Soft",
    }
}

fn format_target(t: &ClaimTarget) -> String {
    match t {
        ClaimTarget::Global => "Global".to_string(),
        ClaimTarget::File { path } => format!("file:{}", truncate(&path.to_string_lossy(), 14)),
        ClaimTarget::Region {
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

fn count_by_kind(claims: &[(&Claim, u64)]) -> String {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (c, _) in claims {
        *map.entry(kind_label(c.kind)).or_insert(0) += 1;
    }
    map.into_iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn style_for(kind: ClaimKind, remaining: u64, theme: &Theme) -> Style {
    // Base color by kind; intensity modulated by remaining TTL.
    let color = match kind {
        ClaimKind::ExclusiveWrite => theme.error,
        ClaimKind::SharedRead => theme.accent,
        ClaimKind::AppendOnly => theme.success,
        ClaimKind::Soft => theme.border,
    };
    let mut style = Style::default().fg(color);
    if kind == ClaimKind::Soft {
        style = style.add_modifier(Modifier::DIM);
    } else if remaining >= 2 {
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
    use nit_core::substrate::{Claim, ClaimKind, ClaimTarget, SubstrateState};
    use std::path::PathBuf;

    fn mk_state_with_claims(claims: Vec<Claim>) -> AppState {
        use nit_core::buffer::Buffer;
        let root = std::env::temp_dir().join(format!(
            "nit-claims-view-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut state =
            AppState::new(root, Buffer::empty("x", None), Buffer::empty("n", None));
        let mut substrate = SubstrateState::default();
        for c in claims {
            substrate.claims.insert(c.id.clone(), c);
        }
        state.substrate = substrate;
        state
    }

    fn mk_claim(id: &str, kind: ClaimKind, gen: u64, ttl: u64) -> Claim {
        Claim {
            id: id.into(),
            kind,
            target: ClaimTarget::File {
                path: PathBuf::from("a.rs"),
            },
            claimed_by: "agent-a".into(),
            claimed_at_gen: gen,
            ttl_gens: ttl,
            rationale: "test".into(),
        }
    }

    #[test]
    fn build_lines_empty_has_header_and_hint() {
        let state = mk_state_with_claims(vec![]);
        let theme = Theme::default();
        let lines = build_lines(&state, &theme, 100);
        // summary + blank + column header + blank + empty hint = 5 lines
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn build_lines_with_two_claims_emits_rows() {
        let claims = vec![
            mk_claim("c1", ClaimKind::ExclusiveWrite, 0, 5),
            mk_claim("c2", ClaimKind::SharedRead, 0, 3),
        ];
        let state = mk_state_with_claims(claims);
        let theme = Theme::default();
        let lines = build_lines(&state, &theme, 100);
        // summary + blank + column header + 2 rows = 5 lines
        assert_eq!(lines.len(), 5);
    }
}
