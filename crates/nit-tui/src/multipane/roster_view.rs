//! Per-pane roster picker. Mirrors the Agent OPS Roster tab so a pane
//! shows the same Backend → Agent tree, Size checkboxes, Template /
//! Mission word toggles, and click-to-pick affordances. State lives on
//! `PaneSession` (cursor, viewport scroll, expand/collapse sets) so two
//! panes can hold independent roster expansions without bleeding into
//! each other or into the Agent OPS dock.
//!
//! Filter shape:
//! - `None` ⇒ show every backend, grouped by family.
//! - `Some(family)` ⇒ show only lanes whose group matches one of the
//!   reserved aliases (`codex`, `claude`, `gemini`, `local`).
//! - `Some(specific-id)` ⇒ show only the matching lane (used by the
//!   `--backend <id>` pre-pick path; the pane is already committed but
//!   Ctrl+R still routes here in case the operator wants to change focus
//!   inside the same family).
//!
//! Rows are computed once per render; the runtime calls [`compute_rows`]
//! again from key/mouse handlers to map cursor moves and click positions
//! to a [`PaneRosterRow`]. Selectable rows include `Backend`, `Agent`,
//! and `SizeLeaf`; `Template` / `Mission` rows are clickable but not
//! cursor-stops (matching Agent OPS).

use nit_core::{
    AgentLane, AgentLaneKind, AgentsState, AppState, PaneSession, RosterTreeBranch,
    RosterTreeSelection,
};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::agent_id::is_multipane_pane_id;
use crate::theme::Theme;

const TEMPLATE_LABEL: &str = " Template: ";
const MISSION_LABEL: &str = " Mission:  ";
pub const TEMPLATE_OPTIONS: [(&str, &str); 3] =
    [("lab", "lab"), ("parallel", "parallel"), ("bulk", "bulk")];
pub const MISSION_OPTIONS: [(&str, &str); 4] = [
    ("auto", "auto"),
    ("general", "general"),
    ("research", "research"),
    ("computational", "computational-research"),
];

/// One semantic row in the per-pane roster. The runtime uses this to
/// route cursor / click events without re-parsing rendered text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneRosterRow {
    /// Word-toggle bar for the swarm template default. Click-only — the
    /// cursor skips it.
    Template,
    /// Word-toggle bar for the swarm mission default. Click-only — the
    /// cursor skips it.
    Mission,
    /// Backend group header with chevron. Selectable; Enter / click
    /// toggles expand.
    Backend { kind: AgentLaneKind },
    /// Concrete agent lane. Selectable; Enter / click commits selection
    /// and materialises a `<base>#mp-pane-NN` lane.
    Agent {
        agent_id: String,
        kind: AgentLaneKind,
        lane_label: String,
    },
    /// "↳ Size" branch under the focused agent. Selectable; Enter
    /// toggles the per-agent tree collapse.
    SizeBranch { agent_id: String },
    /// One reasoning-effort leaf. Selectable; Space / Enter toggles the
    /// checkbox by writing the chosen effort into the relevant
    /// `*_selected_effort` map.
    SizeLeaf {
        agent_id: String,
        leaf_idx: usize,
        effort: String,
        checked: bool,
    },
    /// Empty-state row. Not selectable.
    Empty(String),
    /// Spacer / divider line. Not selectable.
    Spacer,
}

impl PaneRosterRow {
    /// Cursor stops only on Backend and Agent rows. Size / Role leaves
    /// remain rendered (and click-toggleable) but the j/k cursor walker
    /// skips them — auto-expand under the cursor reveals them, and a
    /// click on an Agent row commits the selection. Template / Mission
    /// stay click-only by design.
    pub fn is_selectable(&self) -> bool {
        matches!(
            self,
            PaneRosterRow::Backend { .. } | PaneRosterRow::Agent { .. }
        )
    }
}

/// Build the ordered row list for the pane's roster. The cursor walks
/// only `is_selectable()` rows; renderers iterate every row.
pub fn compute_rows(
    state: &AppState,
    pane: &PaneSession,
    backend_filter: Option<&str>,
) -> Vec<PaneRosterRow> {
    let groups = group_visible_agents(&state.agents, backend_filter);
    let mut rows: Vec<PaneRosterRow> =
        Vec::with_capacity(8 + groups.iter().map(|(_, v)| v.len() * 6).sum::<usize>());

    rows.push(PaneRosterRow::Template);
    rows.push(PaneRosterRow::Mission);
    rows.push(PaneRosterRow::Spacer);

    if groups.is_empty() {
        rows.push(PaneRosterRow::Empty(empty_state_text(backend_filter)));
        return rows;
    }

    for (kind, lanes) in groups {
        rows.push(PaneRosterRow::Backend { kind });
        let backend_expanded = pane.roster_expanded_backends.contains(&kind)
            || pane.auto_expanded_backend == Some(kind);
        if !backend_expanded {
            continue;
        }
        for lane in lanes {
            rows.push(PaneRosterRow::Agent {
                agent_id: lane.id.clone(),
                kind,
                lane_label: lane.lane.clone(),
            });
            let agent_expanded = pane.auto_expanded_agent.as_deref() == Some(lane.id.as_str())
                && !pane.roster_collapsed_agent_ids.contains(&lane.id);
            if !agent_expanded {
                continue;
            }
            let efforts = supported_efforts(state, &lane.id);
            if efforts.is_empty() {
                continue;
            }
            rows.push(PaneRosterRow::SizeBranch {
                agent_id: lane.id.clone(),
            });
            let chosen = chosen_effort(state, pane, &lane.id);
            for (leaf_idx, effort) in efforts.iter().enumerate() {
                rows.push(PaneRosterRow::SizeLeaf {
                    agent_id: lane.id.clone(),
                    leaf_idx,
                    effort: effort.clone(),
                    checked: chosen.as_deref() == Some(effort.as_str()),
                });
            }
        }
    }
    rows
}

/// Number of cursor-stops in `rows`.
pub fn selectable_count(rows: &[PaneRosterRow]) -> usize {
    rows.iter().filter(|r| r.is_selectable()).count()
}

/// Resolve the `cursor`-th selectable row. Returns `None` if the cursor
/// has overshot the available rows (which the runtime then clamps).
pub fn row_at_cursor(rows: &[PaneRosterRow], cursor: usize) -> Option<&PaneRosterRow> {
    rows.iter().filter(|r| r.is_selectable()).nth(cursor)
}

/// Render the roster into `area` from `pane.roster_scroll`, drawing the
/// cursor highlight at `pane.roster_cursor`. Empty states fall through to
/// an inline notice so the pane never blanks out.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    pane: &PaneSession,
    backend_filter: Option<&str>,
    focused: bool,
    theme: &Theme,
) {
    let rows = compute_rows(state, pane, backend_filter);
    let height = area.height.max(1) as usize;
    let max_scroll = rows.len().saturating_sub(height);
    let scroll = pane.roster_scroll.min(max_scroll);
    let cursor_idx = pane.roster_cursor;
    let ctx = RenderCtx {
        state,
        pane,
        rows: &rows,
        cursor_idx,
        focused,
        theme,
    };
    let lines: Vec<Line<'static>> = rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(height)
        .map(|(idx, row)| render_row(&ctx, idx, row))
        .collect();
    let para = Paragraph::new(lines)
        .style(Style::default().bg(crate::widgets::agent_ops_view::ops_table_bg(theme)));
    frame.render_widget(para, area);
}

/// Map a 0-based row offset within the rendered viewport to a row index
/// inside `rows`. `y_offset` is `mouse.row - area.y`. Used by the mouse
/// handler so a click resolves to a `PaneRosterRow` without re-rendering.
pub fn row_index_at_y(rows: &[PaneRosterRow], scroll: usize, y_offset: usize) -> Option<usize> {
    let idx = scroll.checked_add(y_offset)?;
    if idx >= rows.len() {
        return None;
    }
    Some(idx)
}

/// Cursor-index of the `target_row_idx`-th row. Returns `None` if the
/// row is not selectable.
pub fn cursor_for_row_index(rows: &[PaneRosterRow], target_row_idx: usize) -> Option<usize> {
    let mut cursor = 0usize;
    for (idx, row) in rows.iter().enumerate() {
        if !row.is_selectable() {
            continue;
        }
        if idx == target_row_idx {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

/// Click hit-test for the Template line: returns the canonical template
/// value (`lab` / `parallel` / `bulk`) when `col` lies on a word.
pub fn template_word_at_x(col: usize) -> Option<&'static str> {
    word_hit(TEMPLATE_LABEL, &TEMPLATE_OPTIONS, col)
}

/// Click hit-test for the Mission line: returns the canonical mission
/// value (`auto` / `general` / `research` / `computational-research`)
/// when `col` lies on a word.
pub fn mission_word_at_x(col: usize) -> Option<&'static str> {
    word_hit(MISSION_LABEL, &MISSION_OPTIONS, col)
}

/// Click hit-test for the checkbox region of a Size leaf line. Each leaf
/// is rendered with the marker, indent, then `[x]` / `[ ]`. The
/// checkbox occupies a 3-char span. Returns `true` when `col` lands on
/// the checkbox glyphs.
pub fn checkbox_hit_at_x(col: usize) -> bool {
    // Layout: " {marker:1}      {arrow:1} [{checked:1}]" → 11..13 inclusive.
    (11..=13).contains(&col)
}

struct RenderCtx<'a> {
    state: &'a AppState,
    pane: &'a PaneSession,
    rows: &'a [PaneRosterRow],
    cursor_idx: usize,
    focused: bool,
    theme: &'a Theme,
}

fn render_row(ctx: &RenderCtx<'_>, row_idx: usize, row: &PaneRosterRow) -> Line<'static> {
    let highlight = ctx.focused
        && row.is_selectable()
        && cursor_for_row_index(ctx.rows, row_idx) == Some(ctx.cursor_idx);
    match row {
        PaneRosterRow::Template => template_line(ctx.pane, ctx.theme),
        PaneRosterRow::Mission => mission_line(ctx.pane, ctx.theme),
        PaneRosterRow::Backend { kind } => backend_line(*kind, ctx.pane, highlight, ctx.theme),
        PaneRosterRow::Agent {
            agent_id,
            lane_label,
            ..
        } => agent_line(agent_id, lane_label, highlight, ctx.theme),
        PaneRosterRow::SizeBranch { agent_id } => {
            size_branch_line(ctx.state, ctx.pane, agent_id, highlight, ctx.theme)
        }
        PaneRosterRow::SizeLeaf {
            effort, checked, ..
        } => size_leaf_line(effort, *checked, highlight, ctx.theme),
        PaneRosterRow::Empty(text) => empty_line(text, ctx.theme),
        PaneRosterRow::Spacer => Line::from(""),
    }
}

fn word_hit(
    prefix: &'static str,
    words: &'static [(&'static str, &'static str)],
    col: usize,
) -> Option<&'static str> {
    let mut cursor = prefix.chars().count();
    for (display, value) in words {
        let pad_left = 1usize;
        let pad_right = 1usize;
        let token_len = display.chars().count() + pad_left + pad_right;
        let start = cursor;
        let end = start + token_len;
        if col >= start && col < end {
            return Some(value);
        }
        cursor = end + 1; // single-space separator between selectable words
    }
    None
}

fn template_line(pane: &PaneSession, theme: &Theme) -> Line<'static> {
    word_toggle_line(
        TEMPLATE_LABEL,
        &TEMPLATE_OPTIONS,
        &pane.swarm_template,
        theme,
    )
}

fn mission_line(pane: &PaneSession, theme: &Theme) -> Line<'static> {
    word_toggle_line(MISSION_LABEL, &MISSION_OPTIONS, &pane.swarm_mission, theme)
}

fn word_toggle_line(
    prefix: &'static str,
    words: &'static [(&'static str, &'static str)],
    selected_value: &str,
    theme: &Theme,
) -> Line<'static> {
    let label_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let selected_style = Style::default()
        .fg(theme.background)
        .bg(theme.border_focused)
        .add_modifier(Modifier::BOLD);
    let unselected_style = Style::default().fg(theme.foreground);
    let sep_style = label_style;

    let mut spans: Vec<Span<'static>> = Vec::with_capacity(2 + words.len() * 2);
    spans.push(Span::styled(prefix.to_string(), label_style));
    for (idx, (display, value)) in words.iter().enumerate() {
        let style = if selected_value.eq_ignore_ascii_case(value) {
            selected_style
        } else {
            unselected_style
        };
        spans.push(Span::styled(format!(" {display} "), style));
        if idx + 1 < words.len() {
            spans.push(Span::styled(" ", sep_style));
        }
    }
    Line::from(spans)
}

fn backend_line(
    kind: AgentLaneKind,
    pane: &PaneSession,
    highlight: bool,
    theme: &Theme,
) -> Line<'static> {
    let expanded = pane.roster_expanded_backends.contains(&kind);
    let chevron = if expanded { '▾' } else { '▸' };
    let marker = if highlight { '➜' } else { ' ' };
    let label = backend_label(kind);
    let title_style = if highlight {
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::BOLD)
    };
    let chev_style = Style::default().fg(theme.accent);
    Line::from(vec![
        Span::styled(format!(" {marker} "), title_style),
        Span::styled(format!("{chevron} "), chev_style),
        Span::styled(label.to_string(), title_style),
    ])
}

fn agent_line(agent_id: &str, lane_label: &str, highlight: bool, theme: &Theme) -> Line<'static> {
    let marker = if highlight { '➜' } else { '↳' };
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
        Span::styled(format!("   {marker} "), id_style),
        Span::styled(agent_id.to_string(), id_style),
        Span::styled("  ".to_string(), lane_style),
        Span::styled(format!("[{lane_label}]"), lane_style),
    ])
}

fn size_branch_line(
    state: &AppState,
    pane: &PaneSession,
    agent_id: &str,
    highlight: bool,
    theme: &Theme,
) -> Line<'static> {
    let marker = if highlight { '➜' } else { ' ' };
    let collapsed = pane.roster_collapsed_agent_ids.contains(agent_id);
    let chev = if collapsed { '▸' } else { '▾' };
    let label_style = if highlight {
        Style::default().fg(theme.foreground)
    } else {
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM)
    };
    let chosen = chosen_effort(state, pane, agent_id).unwrap_or_default();
    let summary = if chosen.is_empty() {
        String::new()
    } else {
        format!("  ({chosen})")
    };
    Line::from(vec![
        Span::styled(format!(" {marker} "), label_style),
        Span::styled("    ", Style::default()),
        Span::styled(format!("{chev} Size"), label_style),
        Span::styled(summary, label_style),
    ])
}

fn size_leaf_line(effort: &str, checked: bool, highlight: bool, theme: &Theme) -> Line<'static> {
    let marker = if highlight { '➜' } else { ' ' };
    let box_glyph = if checked { 'x' } else { ' ' };
    let leaf_style = if checked {
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD)
    } else if highlight {
        Style::default().fg(theme.foreground)
    } else {
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM)
    };
    let box_style = if checked {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM)
    };
    Line::from(vec![
        Span::styled(format!(" {marker} "), leaf_style),
        Span::styled("      ".to_string(), Style::default()),
        Span::styled(format!("[{box_glyph}]"), box_style),
        Span::styled(format!(" {effort}"), leaf_style),
    ])
}

fn empty_line(text: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::ITALIC),
    ))
}

fn empty_state_text(backend_filter: Option<&str>) -> String {
    let label = backend_filter
        .and_then(family_to_kind)
        .map(backend_label)
        .map(str::to_string)
        .or_else(|| backend_filter.map(str::to_string))
        .unwrap_or_else(|| "agent".into());
    format!(" No {label} agents detected — install the CLI ")
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

fn supported_efforts(state: &AppState, agent_id: &str) -> Vec<String> {
    state
        .agents
        .codex_supported_reasoning_efforts
        .get(agent_id)
        .or_else(|| state.agents.claude_supported_efforts.get(agent_id))
        .cloned()
        .unwrap_or_default()
}

fn chosen_effort(state: &AppState, pane: &PaneSession, agent_id: &str) -> Option<String> {
    if let Some(value) = pane.selected_effort.get(agent_id) {
        return Some(value.clone());
    }
    state
        .agents
        .codex_selected_reasoning_effort
        .get(agent_id)
        .or_else(|| state.agents.codex_default_reasoning_effort.get(agent_id))
        .or_else(|| state.agents.claude_selected_effort.get(agent_id))
        .or_else(|| state.agents.claude_default_effort.get(agent_id))
        .cloned()
}

/// Toggle the checkbox on a Size leaf for the pane at `pane_idx`. Writes
/// only to the pane-local `selected_effort` map; the global
/// `*_selected_effort` maps stay untouched until `dispatch_pane_prompt`
/// bridges the pane choice into the materialised lane id at dispatch
/// time. Returns `true` if any state was changed (false when the lane
/// is missing or the effort isn't in the supported list).
pub fn toggle_size_leaf(
    state: &mut AppState,
    pane_idx: usize,
    agent_id: &str,
    leaf_idx: usize,
) -> bool {
    let efforts = supported_efforts(state, agent_id);
    let Some(effort) = efforts.get(leaf_idx).cloned() else {
        return false;
    };
    let Some(lane) = state.agents.agents.iter().find(|l| l.id == agent_id) else {
        return false;
    };
    let is_codex = lane.is_codex() || matches!(lane.kind, AgentLaneKind::Codex);
    let is_claude = matches!(lane.kind, AgentLaneKind::Claude);
    if !(is_codex || is_claude) {
        return false;
    }
    let Some(mp) = state.multipane.as_mut() else {
        return false;
    };
    let Some(pane) = mp.panes.get_mut(pane_idx) else {
        return false;
    };
    pane.selected_effort.insert(agent_id.to_string(), effort);
    true
}

/// Toggle the expand state of `kind` in the focused pane's roster. Used
/// by the Backend row and the Backend chevron click.
pub fn toggle_backend_expansion(pane: &mut PaneSession, kind: AgentLaneKind) {
    if !pane.roster_expanded_backends.insert(kind) {
        pane.roster_expanded_backends.remove(&kind);
    }
}

/// Toggle the per-agent tree collapse for `agent_id`. Used by the
/// SizeBranch row (Enter / click).
pub fn toggle_agent_tree_collapse(pane: &mut PaneSession, agent_id: &str) {
    if !pane.roster_collapsed_agent_ids.insert(agent_id.to_string()) {
        pane.roster_collapsed_agent_ids.remove(agent_id);
    }
}

/// Update `pane.roster_tree_selected` from the cursor position so the
/// Agent OPS-style leaf selection mirrors the multipane cursor when it
/// lands on a Size leaf. Cleared when the cursor leaves the leaf.
pub fn sync_tree_selection(pane: &mut PaneSession, row: Option<&PaneRosterRow>) {
    pane.roster_tree_selected = match row {
        Some(PaneRosterRow::SizeLeaf { leaf_idx, .. }) => Some(RosterTreeSelection {
            branch: RosterTreeBranch::Size,
            leaf_idx: *leaf_idx,
        }),
        _ => None,
    };
}

/// Drive cursor-driven auto-expansion. Cleared and re-set on every
/// cursor move: a Backend row auto-expands its own group; an Agent row
/// auto-expands both its parent backend and its own Size branch.
/// Anything else (or a None cursor) collapses both auto-fields back to
/// `None`. This is the per-pane single-element latch that gives the
/// "every pass through the roster auto-expandable, leaving collapses"
/// behavior the operator asked for.
pub fn sync_auto_expansion(pane: &mut PaneSession, row: Option<&PaneRosterRow>) {
    match row {
        Some(PaneRosterRow::Backend { kind }) => {
            pane.auto_expanded_backend = Some(*kind);
            pane.auto_expanded_agent = None;
        }
        Some(PaneRosterRow::Agent { agent_id, kind, .. }) => {
            pane.auto_expanded_backend = Some(*kind);
            pane.auto_expanded_agent = Some(agent_id.clone());
        }
        _ => {
            pane.auto_expanded_backend = None;
            pane.auto_expanded_agent = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nit_core::{AgentLane, AgentLaneKind, AgentStatus, AgentsState, AppState, Buffer};
    use std::path::PathBuf;

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

    fn fixture_state() -> AppState {
        let buffer = Buffer::empty("scratch", None);
        let notes = Buffer::empty("notes", None);
        let mut state = AppState::new(PathBuf::from("/workspace"), buffer, notes);
        state.agents = AgentsState::default();
        state.agents.agents = vec![
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
        ];
        state.agents.codex_supported_reasoning_efforts.insert(
            "gpt-5".into(),
            vec!["low".into(), "medium".into(), "high".into()],
        );
        state
            .agents
            .codex_selected_reasoning_effort
            .insert("gpt-5".into(), "medium".into());
        state.agents.claude_supported_efforts.insert(
            "claude-haiku-4-5".into(),
            vec!["low".into(), "medium".into(), "high".into(), "max".into()],
        );
        state.multipane = Some(nit_core::MultipaneState {
            backend_agent_id: String::new(),
            panes: vec![PaneSession::default(), PaneSession::default()],
            focused: 0,
            grid_cols: 2,
            grid_rows: 1,
            backend_filter: None,
        });
        state
    }

    fn pane_with_expansions(expand: &[AgentLaneKind], collapse: &[&str]) -> PaneSession {
        let mut pane = PaneSession::default();
        for k in expand {
            pane.roster_expanded_backends.insert(*k);
        }
        for id in collapse {
            pane.roster_collapsed_agent_ids.insert((*id).into());
        }
        pane
    }

    fn pane_auto_expanded(kind: AgentLaneKind, agent_id: &str) -> PaneSession {
        PaneSession {
            auto_expanded_backend: Some(kind),
            auto_expanded_agent: Some(agent_id.to_string()),
            ..PaneSession::default()
        }
    }

    #[test]
    fn compute_rows_starts_with_template_mission_spacer() {
        let state = fixture_state();
        let pane = PaneSession::default();
        let rows = compute_rows(&state, &pane, None);
        assert!(matches!(rows[0], PaneRosterRow::Template));
        assert!(matches!(rows[1], PaneRosterRow::Mission));
        assert!(matches!(rows[2], PaneRosterRow::Spacer));
    }

    #[test]
    fn compute_rows_collapsed_backends_show_only_headers() {
        let state = fixture_state();
        let pane = PaneSession::default();
        let rows = compute_rows(&state, &pane, None);
        let agent_rows = rows
            .iter()
            .filter(|r| matches!(r, PaneRosterRow::Agent { .. }))
            .count();
        assert_eq!(agent_rows, 0, "no agents until backend expanded");
        let backend_rows = rows
            .iter()
            .filter(|r| matches!(r, PaneRosterRow::Backend { .. }))
            .count();
        assert_eq!(backend_rows, 4, "Codex Claude Gemini Local visible");
    }

    #[test]
    fn compute_rows_expand_codex_shows_lanes_and_size_leaves() {
        let state = fixture_state();
        // Auto-expand both the Codex backend and the gpt-5 agent so size
        // leaves render under the new cursor-driven semantics.
        let pane = pane_auto_expanded(AgentLaneKind::Codex, "gpt-5");
        let rows = compute_rows(&state, &pane, None);
        let agent_ids: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                PaneRosterRow::Agent { agent_id, .. } => Some(agent_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(agent_ids, ["gpt-5"], "only Codex group expanded");

        let size_leaves: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                PaneRosterRow::SizeLeaf { effort, .. } => Some(effort.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(size_leaves, ["low", "medium", "high"]);

        let checked: Vec<bool> = rows
            .iter()
            .filter_map(|r| match r {
                PaneRosterRow::SizeLeaf { checked, .. } => Some(*checked),
                _ => None,
            })
            .collect();
        assert_eq!(checked, [false, true, false], "medium is the chosen effort");
    }

    #[test]
    fn compute_rows_auto_expanded_backend_alone_hides_size_leaves() {
        let state = fixture_state();
        let pane = PaneSession {
            auto_expanded_backend: Some(AgentLaneKind::Codex),
            ..PaneSession::default()
        };
        let rows = compute_rows(&state, &pane, None);
        let agent_count = rows
            .iter()
            .filter(|r| matches!(r, PaneRosterRow::Agent { .. }))
            .count();
        assert_eq!(agent_count, 1, "agents render when backend auto-expanded");
        let leaf_count = rows
            .iter()
            .filter(|r| matches!(r, PaneRosterRow::SizeLeaf { .. }))
            .count();
        assert_eq!(
            leaf_count, 0,
            "size leaves stay hidden until the agent itself is auto-expanded"
        );
    }

    #[test]
    fn compute_rows_collapsed_agent_skips_size_leaves() {
        let state = fixture_state();
        let pane = pane_with_expansions(&[AgentLaneKind::Codex], &["gpt-5"]);
        let rows = compute_rows(&state, &pane, None);
        let leaf_count = rows
            .iter()
            .filter(|r| matches!(r, PaneRosterRow::SizeLeaf { .. }))
            .count();
        assert_eq!(leaf_count, 0);
    }

    #[test]
    fn selectable_count_excludes_template_mission_spacer_empty() {
        let state = fixture_state();
        let pane = PaneSession::default();
        let rows = compute_rows(&state, &pane, None);
        // 4 backend rows (Codex Claude Gemini Local), no agent expand.
        assert_eq!(selectable_count(&rows), 4);
    }

    #[test]
    fn cursor_for_row_index_skips_non_selectable() {
        let state = fixture_state();
        let pane = pane_with_expansions(&[AgentLaneKind::Codex], &[]);
        let rows = compute_rows(&state, &pane, None);
        // Find the Agent { gpt-5 } row.
        let target_idx = rows
            .iter()
            .position(|r| matches!(r, PaneRosterRow::Agent { agent_id, .. } if agent_id == "gpt-5"))
            .expect("gpt-5 row");
        let cursor = cursor_for_row_index(&rows, target_idx).expect("cursor");
        // Skipping Template, Mission, Spacer (3 leading rows) and Backend Codex (1 backend),
        // gpt-5 is the 2nd selectable row → cursor index 1.
        assert_eq!(cursor, 1);
    }

    #[test]
    fn template_word_at_x_resolves_offset() {
        // " Template:  lab   parallel   bulk " — first word " lab " starts after prefix.
        let prefix_len = TEMPLATE_LABEL.chars().count();
        let lab_start = prefix_len;
        assert_eq!(template_word_at_x(lab_start), Some("lab"));
        assert_eq!(template_word_at_x(lab_start + 1), Some("lab"));
        // The selectable token includes pad spaces (" lab "), so the trailing space is still a hit
        let lab_token_end = prefix_len + " lab ".chars().count();
        assert_eq!(template_word_at_x(lab_token_end - 1), Some("lab"));
        // After the single-space separator we land on " parallel "
        let parallel_start = lab_token_end + 1;
        assert_eq!(template_word_at_x(parallel_start), Some("parallel"));
        assert_eq!(template_word_at_x(parallel_start + 5), Some("parallel"));
    }

    #[test]
    fn mission_word_at_x_resolves_computational() {
        let prefix_len = MISSION_LABEL.chars().count();
        let auto_token_len = " auto ".chars().count();
        let general_token_len = " general ".chars().count();
        let research_token_len = " research ".chars().count();
        // After auto + sep + general + sep + research + sep we land on " computational "
        let comp_start =
            prefix_len + auto_token_len + 1 + general_token_len + 1 + research_token_len + 1;
        assert_eq!(
            mission_word_at_x(comp_start),
            Some("computational-research")
        );
    }

    #[test]
    fn toggle_size_leaf_writes_to_codex_selected_effort() {
        let mut state = fixture_state();
        let toggled = toggle_size_leaf(&mut state, 0, "gpt-5", 2);
        assert!(toggled);
        assert_eq!(
            state.multipane.as_ref().unwrap().panes[0]
                .selected_effort
                .get("gpt-5"),
            Some(&"high".to_string())
        );
        assert_eq!(
            state.agents.codex_selected_reasoning_effort.get("gpt-5"),
            Some(&"medium".to_string()),
            "global default seeded by fixture must stay untouched"
        );
    }

    #[test]
    fn toggle_size_leaf_writes_to_claude_selected_effort() {
        let mut state = fixture_state();
        let toggled = toggle_size_leaf(&mut state, 0, "claude-haiku-4-5", 3);
        assert!(toggled);
        assert_eq!(
            state.multipane.as_ref().unwrap().panes[0]
                .selected_effort
                .get("claude-haiku-4-5"),
            Some(&"max".to_string())
        );
        assert!(
            !state
                .agents
                .claude_selected_effort
                .contains_key("claude-haiku-4-5"),
            "global claude_selected_effort must stay untouched"
        );
    }

    #[test]
    fn two_panes_pick_independent_sizes() {
        let mut state = fixture_state();
        assert!(toggle_size_leaf(&mut state, 0, "gpt-5", 0));
        assert!(toggle_size_leaf(&mut state, 1, "gpt-5", 2));
        let panes = &state.multipane.as_ref().unwrap().panes;
        assert_eq!(panes[0].selected_effort.get("gpt-5"), Some(&"low".into()));
        assert_eq!(panes[1].selected_effort.get("gpt-5"), Some(&"high".into()));
    }

    #[test]
    fn toggle_backend_expansion_round_trips() {
        let mut pane = PaneSession::default();
        toggle_backend_expansion(&mut pane, AgentLaneKind::Codex);
        assert!(pane
            .roster_expanded_backends
            .contains(&AgentLaneKind::Codex));
        toggle_backend_expansion(&mut pane, AgentLaneKind::Codex);
        assert!(!pane
            .roster_expanded_backends
            .contains(&AgentLaneKind::Codex));
    }

    #[test]
    fn toggle_agent_tree_collapse_round_trips() {
        let mut pane = PaneSession::default();
        toggle_agent_tree_collapse(&mut pane, "gpt-5");
        assert!(pane.roster_collapsed_agent_ids.contains("gpt-5"));
        toggle_agent_tree_collapse(&mut pane, "gpt-5");
        assert!(!pane.roster_collapsed_agent_ids.contains("gpt-5"));
    }

    #[test]
    fn sync_tree_selection_sets_size_leaf() {
        let mut pane = PaneSession::default();
        let row = PaneRosterRow::SizeLeaf {
            agent_id: "gpt-5".into(),
            leaf_idx: 1,
            effort: "medium".into(),
            checked: true,
        };
        sync_tree_selection(&mut pane, Some(&row));
        assert_eq!(
            pane.roster_tree_selected,
            Some(RosterTreeSelection {
                branch: RosterTreeBranch::Size,
                leaf_idx: 1,
            })
        );

        sync_tree_selection(
            &mut pane,
            Some(&PaneRosterRow::Backend {
                kind: AgentLaneKind::Codex,
            }),
        );
        assert!(pane.roster_tree_selected.is_none());
    }

    #[test]
    fn family_to_kind_resolves_closed_set() {
        assert_eq!(family_to_kind("codex"), Some(AgentLaneKind::Codex));
        assert_eq!(family_to_kind("CLAUDE"), Some(AgentLaneKind::Claude));
        assert_eq!(family_to_kind("Gemini"), Some(AgentLaneKind::Gemini));
        assert_eq!(family_to_kind("local"), Some(AgentLaneKind::Mock));
        assert_eq!(family_to_kind("anthropic"), None);
    }

    #[test]
    fn empty_state_text_uses_filter_label() {
        assert!(empty_state_text(Some("claude")).contains("Claude"));
        assert!(empty_state_text(None).contains("agent"));
    }
}
