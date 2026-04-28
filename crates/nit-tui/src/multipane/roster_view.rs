//! Per-pane roster picker. Renders a filtered, cursor-driven list of
//! agent lanes from `state.agents.agents`. Skips shadow lanes and any
//! lane whose id matches `is_multipane_pane_id` so a pane never lists
//! its own (or another pane's) `#mp-pane-NN` clone as selectable.
//!
//! Filter shape:
//! - `None` ⇒ show every lane, grouped by backend family.
//! - `Some(family)` ⇒ show only lanes whose group matches one of the
//!   reserved aliases (`codex`, `claude`, `gemini`, `local`).
//! - `Some(specific-id)` ⇒ show only the matching lane (used by the
//!   `--backend <id>` pre-pick path; the pane is already committed but
//!   Ctrl+R still routes here in case the operator wants to change
//!   focus inside the same family).

use nit_core::{AgentLane, AgentLaneKind, AgentsState};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::agent_id::is_multipane_pane_id;
use crate::theme::Theme;

/// One renderable row in the pane roster — either a backend group header
/// or a selectable lane row. Returned in display order so the cursor
/// index maps directly to a row position; cursor logic (in `runtime.rs`)
/// only counts `Lane` rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RosterRow {
    Header {
        label: String,
        kind: AgentLaneKind,
    },
    Lane {
        agent_id: String,
        kind: AgentLaneKind,
        lane_label: String,
    },
}

/// Build the ordered roster rows for `state.agents.agents` filtered by
/// `backend_filter`. Headers appear above each non-empty group; an empty
/// rendering returns no rows (caller handles the empty-state row).
pub fn compute_rows(agents: &AgentsState, backend_filter: Option<&str>) -> Vec<RosterRow> {
    let groups = group_visible_agents(agents, backend_filter);
    let mut rows = Vec::with_capacity(groups.iter().map(|(_, v)| v.len() + 1).sum());
    for (kind, lanes) in groups {
        rows.push(RosterRow::Header {
            label: backend_label(kind).to_string(),
            kind,
        });
        for lane in lanes {
            rows.push(RosterRow::Lane {
                agent_id: lane.id.clone(),
                kind,
                lane_label: lane.lane.clone(),
            });
        }
    }
    rows
}

/// Count of selectable lane rows in `rows`. Used by the runtime to clamp
/// the cursor when the roster shrinks (e.g. after a sibling pane
/// commits a selection that adds a `#mp-pane-NN` lane to the canonical
/// roster).
pub fn lane_count(rows: &[RosterRow]) -> usize {
    rows.iter()
        .filter(|r| matches!(r, RosterRow::Lane { .. }))
        .count()
}

/// Resolve the `cursor`-th selectable lane in `rows`, returning the lane
/// id. Header rows are skipped.
pub fn lane_at_cursor(rows: &[RosterRow], cursor: usize) -> Option<&str> {
    rows.iter()
        .filter_map(|r| match r {
            RosterRow::Lane { agent_id, .. } => Some(agent_id.as_str()),
            _ => None,
        })
        .nth(cursor)
}

/// Render the roster into `area`, drawing the cursor highlight at
/// `cursor` (the `cursor`-th lane row). Empty filters fall through to
/// an inline empty-state line so the pane never blanks out.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &nit_core::AppState,
    backend_filter: Option<&str>,
    cursor: usize,
    focused: bool,
    theme: &Theme,
) {
    let rows = compute_rows(&state.agents, backend_filter);
    let lines = if rows.is_empty() {
        vec![empty_state_line(backend_filter, theme)]
    } else {
        let height = area.height.max(1) as usize;
        let scroll = scroll_offset_for_cursor(&rows, cursor, height);
        format_lines(&rows, cursor, scroll, height, focused, theme)
    };
    let para = Paragraph::new(lines).style(Style::default().bg(theme.background));
    frame.render_widget(para, area);
}

fn group_visible_agents<'a>(
    agents: &'a AgentsState,
    backend_filter: Option<&str>,
) -> Vec<(AgentLaneKind, Vec<&'a AgentLane>)> {
    let mut codex: Vec<&AgentLane> = Vec::new();
    let mut claude: Vec<&AgentLane> = Vec::new();
    let mut gemini: Vec<&AgentLane> = Vec::new();
    let mut local: Vec<&AgentLane> = Vec::new();
    let mut other: Vec<&AgentLane> = Vec::new();

    for lane in &agents.agents {
        if lane.shadow || is_multipane_pane_id(&lane.id) {
            continue;
        }
        let kind = group_for_lane(lane);
        if !filter_matches(backend_filter, kind, &lane.id) {
            continue;
        }
        match kind {
            AgentLaneKind::Codex => codex.push(lane),
            AgentLaneKind::Claude => claude.push(lane),
            AgentLaneKind::Gemini => gemini.push(lane),
            AgentLaneKind::Mock => local.push(lane),
            AgentLaneKind::Unknown => other.push(lane),
        }
    }

    let mut out: Vec<(AgentLaneKind, Vec<&AgentLane>)> = Vec::with_capacity(5);
    for (kind, lanes) in [
        (AgentLaneKind::Codex, codex),
        (AgentLaneKind::Claude, claude),
        (AgentLaneKind::Gemini, gemini),
        (AgentLaneKind::Mock, local),
        (AgentLaneKind::Unknown, other),
    ] {
        if !lanes.is_empty() {
            out.push((kind, lanes));
        }
    }
    out
}

fn filter_matches(backend_filter: Option<&str>, kind: AgentLaneKind, lane_id: &str) -> bool {
    let Some(value) = backend_filter else {
        return true;
    };
    if let Some(family_kind) = family_to_kind(value) {
        return family_kind == kind;
    }
    lane_id == value
}

fn family_to_kind(value: &str) -> Option<AgentLaneKind> {
    match value.to_ascii_lowercase().as_str() {
        "codex" => Some(AgentLaneKind::Codex),
        "claude" => Some(AgentLaneKind::Claude),
        "gemini" => Some(AgentLaneKind::Gemini),
        "local" => Some(AgentLaneKind::Mock),
        _ => None,
    }
}

fn group_for_lane(lane: &AgentLane) -> AgentLaneKind {
    if lane.is_codex() {
        AgentLaneKind::Codex
    } else if matches!(lane.kind, AgentLaneKind::Claude) {
        AgentLaneKind::Claude
    } else if matches!(lane.kind, AgentLaneKind::Gemini) {
        AgentLaneKind::Gemini
    } else if matches!(lane.kind, AgentLaneKind::Mock) {
        AgentLaneKind::Mock
    } else {
        AgentLaneKind::Unknown
    }
}

fn backend_label(kind: AgentLaneKind) -> &'static str {
    match kind {
        AgentLaneKind::Codex => "Codex",
        AgentLaneKind::Claude => "Claude",
        AgentLaneKind::Gemini => "Gemini",
        AgentLaneKind::Mock => "Local",
        AgentLaneKind::Unknown => "Other",
    }
}

fn empty_state_line(backend_filter: Option<&str>, theme: &Theme) -> Line<'static> {
    let label = backend_filter
        .and_then(family_to_kind)
        .map(backend_label)
        .map(str::to_string)
        .or_else(|| backend_filter.map(str::to_string))
        .unwrap_or_else(|| "agent".into());
    let text = format!(" No {label} agents detected — install the CLI ");
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::ITALIC),
    ))
}

fn scroll_offset_for_cursor(rows: &[RosterRow], cursor: usize, height: usize) -> usize {
    if height == 0 {
        return 0;
    }
    let cursor_row = lane_cursor_to_row_index(rows, cursor);
    if cursor_row < height {
        return 0;
    }
    cursor_row + 1 - height
}

fn lane_cursor_to_row_index(rows: &[RosterRow], cursor: usize) -> usize {
    let mut lanes_seen = 0usize;
    for (idx, row) in rows.iter().enumerate() {
        if matches!(row, RosterRow::Lane { .. }) {
            if lanes_seen == cursor {
                return idx;
            }
            lanes_seen += 1;
        }
    }
    rows.len().saturating_sub(1)
}

fn format_lines(
    rows: &[RosterRow],
    cursor: usize,
    scroll: usize,
    height: usize,
    focused: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(height);
    let mut lane_idx = 0usize;
    for row in rows.iter().skip(scroll).take(height) {
        match row {
            RosterRow::Header { label, .. } => {
                lines.push(header_line(label, theme));
            }
            RosterRow::Lane {
                agent_id,
                lane_label,
                ..
            } => {
                let highlight = focused && lane_idx + scroll_lane_offset(rows, scroll) == cursor;
                lines.push(lane_line(agent_id, lane_label, highlight, theme));
                lane_idx += 1;
            }
        }
    }
    lines
}

fn scroll_lane_offset(rows: &[RosterRow], scroll: usize) -> usize {
    rows.iter()
        .take(scroll)
        .filter(|r| matches!(r, RosterRow::Lane { .. }))
        .count()
}

fn header_line(label: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!(" ▾ {label}"),
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::BOLD),
    ))
}

fn lane_line(agent_id: &str, lane_label: &str, highlight: bool, theme: &Theme) -> Line<'static> {
    let marker = if highlight { "▶" } else { " " };
    let id_style = if highlight {
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.foreground)
    };
    let lane_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    Line::from(vec![
        Span::styled(format!(" {marker} "), id_style),
        Span::styled(agent_id.to_string(), id_style),
        Span::styled("  ".to_string(), lane_style),
        Span::styled(format!("[{lane_label}]"), lane_style),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use nit_core::{AgentLane, AgentLaneKind, AgentStatus, AgentsState};

    fn lane(id: &str, lane_label: &str, kind: AgentLaneKind) -> AgentLane {
        AgentLane {
            id: id.into(),
            role: id.into(),
            lane: lane_label.into(),
            kind,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
            shadow: false,
        }
    }

    fn shadow_lane(id: &str, kind: AgentLaneKind) -> AgentLane {
        let mut l = lane(id, "shadow", kind);
        l.shadow = true;
        l
    }

    fn fixture_agents() -> AgentsState {
        AgentsState {
            agents: vec![
                lane("claude-haiku-4-5", "Claude", AgentLaneKind::Claude),
                lane("claude-opus-4-6", "Claude", AgentLaneKind::Claude),
                lane("gpt-5", "Codex", AgentLaneKind::Codex),
                lane("gemini-2.5-pro", "Gemini", AgentLaneKind::Gemini),
                lane("local", "Local", AgentLaneKind::Mock),
                shadow_lane("claude-haiku-4-5#shadow-x", AgentLaneKind::Claude),
                lane(
                    "claude-haiku-4-5#mp-pane-00",
                    "Claude",
                    AgentLaneKind::Claude,
                ),
            ],
            ..AgentsState::default()
        }
    }

    #[test]
    fn compute_rows_groups_by_family_and_skips_shadow_and_pane_lanes() {
        let agents = fixture_agents();
        let rows = compute_rows(&agents, None);
        let lane_ids: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                RosterRow::Lane { agent_id, .. } => Some(agent_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            lane_ids,
            [
                "gpt-5",
                "claude-haiku-4-5",
                "claude-opus-4-6",
                "gemini-2.5-pro",
                "local",
            ]
        );
        // No header → lane uniqueness violations and headers ordered by group.
        let headers: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                RosterRow::Header { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(headers, ["Codex", "Claude", "Gemini", "Local"]);
    }

    #[test]
    fn compute_rows_filters_by_family_alias() {
        let agents = fixture_agents();
        let rows = compute_rows(&agents, Some("claude"));
        let ids: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                RosterRow::Lane { agent_id, .. } => Some(agent_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, ["claude-haiku-4-5", "claude-opus-4-6"]);
    }

    #[test]
    fn compute_rows_filters_by_specific_lane_id() {
        let agents = fixture_agents();
        let rows = compute_rows(&agents, Some("gpt-5"));
        let ids: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                RosterRow::Lane { agent_id, .. } => Some(agent_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, ["gpt-5"]);
    }

    #[test]
    fn compute_rows_returns_empty_when_filter_matches_no_lane() {
        let agents = fixture_agents();
        let rows = compute_rows(&agents, Some("gemini-pretendmodel"));
        assert!(rows.is_empty());
    }

    #[test]
    fn lane_at_cursor_skips_headers() {
        let agents = fixture_agents();
        let rows = compute_rows(&agents, None);
        assert_eq!(lane_at_cursor(&rows, 0), Some("gpt-5"));
        assert_eq!(lane_at_cursor(&rows, 1), Some("claude-haiku-4-5"));
        assert_eq!(lane_at_cursor(&rows, 4), Some("local"));
        assert_eq!(lane_at_cursor(&rows, 99), None);
    }

    #[test]
    fn lane_count_excludes_headers() {
        let agents = fixture_agents();
        let rows = compute_rows(&agents, None);
        assert_eq!(lane_count(&rows), 5);
    }

    #[test]
    fn family_to_kind_resolves_closed_set() {
        assert_eq!(family_to_kind("codex"), Some(AgentLaneKind::Codex));
        assert_eq!(family_to_kind("CLAUDE"), Some(AgentLaneKind::Claude));
        assert_eq!(family_to_kind("Gemini"), Some(AgentLaneKind::Gemini));
        assert_eq!(family_to_kind("local"), Some(AgentLaneKind::Mock));
        assert_eq!(family_to_kind("anthropic"), None);
        assert_eq!(family_to_kind(""), None);
    }
}
