use nit_core::{
    AgentAlertSeverity, AgentLaneKind, AgentMessage, AgentOpsTab, AgentStatus, AppKind, AppState,
    EvidenceItem, GlobalArchiveEntry, GlobalArchiveSourceKind, McpConnectionState, PaneId,
    RosterTreeBranch, RosterTreeSelection, SavedRunHistoryFilter, UiSelectionPane,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use time::OffsetDateTime;

use crate::swarm::{
    chat_clone_base_id, is_chat_clone_agent_id, normalize_role_label, GateReportGate,
    SwarmDashboardView, SwarmPersistenceView, SwarmRuntime, SwarmTaskArtifacts,
};
use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;

pub fn mission_index_for_body_line(state: &AppState, body_line: usize) -> Option<usize> {
    mission_body_meta(state, body_line).map(|meta| meta.mission_idx)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RosterBodyNode {
    Backend {
        backend: AgentLaneKind,
    },
    Agent,
    Branch {
        branch: RosterTreeBranch,
    },
    Leaf {
        branch: RosterTreeBranch,
        leaf_idx: usize,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RosterSelectableRow {
    Backend { backend: AgentLaneKind },
    Agent { agent_idx: usize },
}

pub struct RosterBodyMeta {
    pub agent_idx: Option<usize>,
    pub node: RosterBodyNode,
}

pub fn roster_meta_for_body_line(state: &AppState, body_line: usize) -> Option<RosterBodyMeta> {
    roster_body_meta(state, body_line)
}

pub fn roster_body_offset(state: &AppState) -> usize {
    roster_header_offsets(state).body_offset
}

pub fn roster_selection_rows(state: &AppState) -> Vec<RosterSelectableRow> {
    let mut out = Vec::with_capacity(state.agents.agents.len().saturating_add(5));
    for (backend, agent_indices) in roster_grouped_agent_indices(state) {
        out.push(RosterSelectableRow::Backend { backend });
        if !roster_backend_is_expanded(state, backend) {
            continue;
        }
        out.extend(
            agent_indices
                .into_iter()
                .map(|agent_idx| RosterSelectableRow::Agent { agent_idx }),
        );
    }
    out
}

pub fn roster_selected_row(state: &AppState) -> Option<RosterSelectableRow> {
    let grouped = roster_grouped_agent_indices(state);
    if let Some(backend) = state.agents.roster_selected_backend {
        if grouped.iter().any(|(kind, _)| *kind == backend) {
            return Some(RosterSelectableRow::Backend { backend });
        }
    }

    if roster_agent_display_order(state).contains(&state.agents.roster_selected) {
        return Some(RosterSelectableRow::Agent {
            agent_idx: state.agents.roster_selected,
        });
    }

    if let Some(agent) = state.agents.agents.get(state.agents.roster_selected) {
        let backend = roster_backend_group_for_agent(agent);
        if grouped.iter().any(|(kind, _)| *kind == backend) {
            return Some(RosterSelectableRow::Backend { backend });
        }
    }

    grouped
        .into_iter()
        .next()
        .map(|(backend, _)| RosterSelectableRow::Backend { backend })
}

pub fn roster_swarm_template_line_idx(state: &AppState) -> usize {
    roster_header_offsets(state).template_line
}

pub fn roster_swarm_mission_line_idx(state: &AppState) -> usize {
    roster_header_offsets(state).mission_line
}

const ROSTER_BACKEND_NAME_W: usize = 7;
const ROSTER_SWARM_TEMPLATE_LINE: &str = " Template:  lab   parallel   bulk ";
const ROSTER_SWARM_MISSION_LINE: &str = " Mission:   auto   general   research   computational ";
const ROSTER_ROLE_OPTIONS: [&str; 8] = [
    "all",
    "propose",
    "research",
    "computational-research",
    "judge",
    "integrate",
    "review",
    "test",
];

pub fn roster_swarm_template_hit(col: usize) -> Option<&'static str> {
    for label in ["lab", "parallel", "bulk"] {
        let needle = match label {
            "lab" => " lab ",
            "parallel" => " parallel ",
            "bulk" => " bulk ",
            _ => continue,
        };
        let Some(start) = ROSTER_SWARM_TEMPLATE_LINE.find(needle) else {
            continue;
        };
        let end = start.saturating_add(needle.len());
        if col >= start && col < end {
            return Some(label);
        }
    }
    None
}

pub fn roster_swarm_mission_hit(col: usize) -> Option<&'static str> {
    for (label, value) in [
        ("auto", "auto"),
        ("general", "general"),
        ("research", "research"),
        ("computational", "computational-research"),
    ] {
        let needle = format!(" {label} ");
        let Some(start) = ROSTER_SWARM_MISSION_LINE.find(needle.as_str()) else {
            continue;
        };
        let end = start.saturating_add(needle.len());
        if col >= start && col < end {
            return Some(value);
        }
    }
    None
}

pub fn roster_role_cell_hit(col: usize, width: usize) -> bool {
    let widths = roster_column_widths(width.max(32));
    let start = 1usize; // selection marker prefix
    let end = start.saturating_add(widths.first().copied().unwrap_or(0));
    col >= start && col < end
}

const ARROW_PRIMARY: char = '↳';
const ARROW_FALLBACK: char = '>';
const CURSOR_PRIMARY: char = '➜';
const CURSOR_FALLBACK: char = '>';
const TREE_CLOSED_PRIMARY: char = '▸';
const TREE_CLOSED_FALLBACK: char = '>';
const TREE_OPEN_PRIMARY: char = '▾';
const TREE_OPEN_FALLBACK: char = 'v';

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BackendInventoryBackend {
    Codex,
    Claude,
    Gemini,
    Local,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct BackendInventoryRow {
    backend: BackendInventoryBackend,
    available: bool,
    active: bool,
}

fn roster_inventory_backend_accent(backend: BackendInventoryBackend, theme: &Theme) -> Color {
    match backend {
        BackendInventoryBackend::Codex => theme.title,
        BackendInventoryBackend::Claude => theme.border_focused,
        BackendInventoryBackend::Gemini => theme.accent,
        BackendInventoryBackend::Local => theme.border,
    }
}

fn roster_lane_backend_accent(backend: AgentLaneKind, theme: &Theme) -> Color {
    match backend {
        AgentLaneKind::Codex => theme.title,
        AgentLaneKind::Claude => theme.border_focused,
        AgentLaneKind::Gemini => theme.accent,
        AgentLaneKind::Mock => theme.border,
        AgentLaneKind::Unknown => theme.title,
    }
}

struct RosterHeaderOffsets {
    blank_after_backends: usize,
    template_line: usize,
    mission_line: usize,
    blank_after_mission: usize,
    table_header: usize,
    table_separator: usize,
    body_offset: usize,
}

fn roster_backend_inventory_rows(state: &AppState) -> Vec<BackendInventoryRow> {
    let codex_available = state.agents.codex_cli_available
        || state.agents.agents.iter().any(|agent| agent.is_codex());
    let codex_active = codex_available;

    let claude_active = state
        .agents
        .agents
        .iter()
        .any(|agent| matches!(agent.kind, AgentLaneKind::Claude));
    let claude_available = state.agents.claude_cli_available || claude_active;
    let claude_active = claude_available;

    let gemini_active = state
        .agents
        .agents
        .iter()
        .any(|agent| matches!(agent.kind, AgentLaneKind::Gemini));
    let gemini_available = state.agents.gemini_cli_available || gemini_active;
    let gemini_active = gemini_available;

    let local_active = state
        .agents
        .agents
        .iter()
        .any(|agent| matches!(agent.kind, AgentLaneKind::Mock));

    let out = vec![
        BackendInventoryRow {
            backend: BackendInventoryBackend::Codex,
            available: codex_available,
            active: codex_active,
        },
        BackendInventoryRow {
            backend: BackendInventoryBackend::Claude,
            available: claude_available,
            active: claude_active,
        },
        BackendInventoryRow {
            backend: BackendInventoryBackend::Gemini,
            available: gemini_available,
            active: gemini_active,
        },
        BackendInventoryRow {
            backend: BackendInventoryBackend::Local,
            available: true,
            active: local_active,
        },
    ];

    out
}

fn roster_header_offsets(state: &AppState) -> RosterHeaderOffsets {
    let backend_rows = roster_backend_inventory_rows(state).len();
    let blank_after_backends = 1usize.saturating_add(backend_rows);
    let template_line = blank_after_backends.saturating_add(1);
    let mission_line = template_line.saturating_add(1);
    let blank_after_mission = mission_line.saturating_add(1);
    let table_header = blank_after_mission.saturating_add(1);
    let table_separator = table_header.saturating_add(1);
    let body_offset = table_separator.saturating_add(1);

    RosterHeaderOffsets {
        blank_after_backends,
        template_line,
        mission_line,
        blank_after_mission,
        table_header,
        table_separator,
        body_offset,
    }
}

fn arrow_glyph() -> char {
    // Allow users/CI to opt into ASCII-safe markers if the font lacks the arrow glyph.
    if std::env::var("NIT_ASCII_FALLBACK").is_ok() {
        ARROW_FALLBACK
    } else {
        ARROW_PRIMARY
    }
}

fn cursor_glyph() -> char {
    // Keep this in lock-step with `arrow_glyph()` so users can flip one env var to avoid Unicode.
    if std::env::var("NIT_ASCII_FALLBACK").is_ok() {
        CURSOR_FALLBACK
    } else {
        CURSOR_PRIMARY
    }
}

fn tree_closed_glyph() -> char {
    if std::env::var("NIT_ASCII_FALLBACK").is_ok() {
        TREE_CLOSED_FALLBACK
    } else {
        TREE_CLOSED_PRIMARY
    }
}

fn tree_open_glyph() -> char {
    if std::env::var("NIT_ASCII_FALLBACK").is_ok() {
        TREE_OPEN_FALLBACK
    } else {
        TREE_OPEN_PRIMARY
    }
}

fn is_swarm_clone_agent_id(agent_id: &str) -> bool {
    agent_id.split_once("#swarm-").is_some()
}

fn swarm_clone_label_parts(agent_id: &str) -> Option<(&str, &str)> {
    let (_base, rest) = agent_id.split_once("#swarm-")?;
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }

    let first_dash = rest.find('-')?;
    let second_dash_rel = rest[first_dash.saturating_add(1)..].find('-')?;
    let second_dash = first_dash.saturating_add(1).saturating_add(second_dash_rel);
    let (mission_id, suffix) = rest.split_at(second_dash);
    let mission_id = mission_id.trim();
    let suffix = suffix.trim_start_matches('-').trim();
    if mission_id.is_empty() || suffix.is_empty() {
        return None;
    }
    Some((mission_id, suffix))
}

fn compact_swarm_clone_suffix(suffix: &str) -> String {
    let parts = suffix
        .split('-')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        suffix.trim().to_string()
    } else {
        parts.join(" ")
    }
}

fn swarm_assigned_roles_for_agent(
    dashboard: &SwarmDashboardView,
    agent_id: &str,
) -> Option<String> {
    let mut roles: Vec<String> = Vec::new();
    for task in dashboard
        .tasks
        .iter()
        .filter(|task| task.agent_id == agent_id)
    {
        let Some(role) = task
            .role
            .as_deref()
            .and_then(normalize_role_label)
            .filter(|role| !role.is_empty())
        else {
            continue;
        };
        if roles.iter().any(|existing| existing == &role) {
            continue;
        }
        roles.push(role);
    }
    if roles.is_empty() {
        None
    } else {
        Some(roles.join("+"))
    }
}

fn swarm_clone_display_label(agent_id: &str, role: Option<&str>) -> Option<String> {
    let (_base, rest) = agent_id.split_once("#swarm-")?;
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }

    let role = role
        .map(str::trim)
        .filter(|role| !role.is_empty())
        .map(normalize_roster_role_hint);

    let Some((_mission_id, suffix)) = swarm_clone_label_parts(agent_id) else {
        let label = compact_swarm_clone_suffix(rest);
        return Some(match role {
            Some(role) => format!("{label} [{role}]"),
            None => label,
        });
    };

    let label = compact_swarm_clone_suffix(suffix);
    Some(match role {
        Some(role) => format!("{label} [{role}]"),
        None => label,
    })
}

fn normalize_roster_role_hint(raw: &str) -> String {
    let role = raw.trim();
    if role.eq_ignore_ascii_case("all") {
        return "all".into();
    }
    normalize_role_label(role).unwrap_or_else(|| role.to_ascii_lowercase())
}

fn table_role_label(role: &str) -> String {
    let role = role.trim();
    if role.is_empty() {
        return String::new();
    }

    // Keep existing role casing/labels, but force a single canonical display spelling for this
    // legacy role variant.
    if normalize_role_label(role).as_deref() == Some("computational-research") {
        return "computational-research".into();
    }

    role.to_string()
}

pub fn alert_index_for_body_line(
    state: &AppState,
    width: usize,
    body_line: usize,
) -> Option<usize> {
    alert_body_meta(state, width, body_line).map(|meta| meta.alert_idx)
}

fn roster_body_meta(state: &AppState, body_line: usize) -> Option<RosterBodyMeta> {
    let show_roles = matches!(
        state
            .agents
            .swarm_default_template
            .to_ascii_lowercase()
            .as_str(),
        "bulk" | "parallel"
    );
    let mut cursor = 0usize;
    for (backend, agent_indices) in roster_grouped_agent_indices(state) {
        if body_line == cursor {
            return Some(RosterBodyMeta {
                agent_idx: None,
                node: RosterBodyNode::Backend { backend },
            });
        }
        cursor = cursor.saturating_add(1);
        if !roster_backend_is_expanded(state, backend) {
            continue;
        }

        for agent_idx in agent_indices {
            let Some(agent) = state.agents.agents.get(agent_idx) else {
                continue;
            };

            if body_line == cursor {
                return Some(RosterBodyMeta {
                    agent_idx: Some(agent_idx),
                    node: RosterBodyNode::Agent,
                });
            }
            cursor = cursor.saturating_add(1);

            if agent_idx == state.agents.roster_selected
                && !is_swarm_clone_agent_id(agent.id.as_str())
                && !state
                    .agents
                    .roster_tree_collapsed_agent_ids
                    .contains(&agent.id)
            {
                let efforts = state
                    .agents
                    .codex_supported_reasoning_efforts
                    .get(&agent.id)
                    .or_else(|| state.agents.claude_supported_efforts.get(&agent.id))
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let has_size = !efforts.is_empty();
                let has_roles = show_roles && (agent.is_codex() || agent.is_claude());

                for branch in [RosterTreeBranch::Size, RosterTreeBranch::Role] {
                    if matches!(branch, RosterTreeBranch::Size) && !has_size {
                        continue;
                    }
                    if matches!(branch, RosterTreeBranch::Role) && !has_roles {
                        continue;
                    }

                    if body_line == cursor {
                        return Some(RosterBodyMeta {
                            agent_idx: Some(agent_idx),
                            node: RosterBodyNode::Branch { branch },
                        });
                    }
                    cursor = cursor.saturating_add(1);

                    let leaf_len = match branch {
                        RosterTreeBranch::Size => efforts.len(),
                        RosterTreeBranch::Role => ROSTER_ROLE_OPTIONS.len(),
                    };
                    if body_line < cursor.saturating_add(leaf_len) {
                        return Some(RosterBodyMeta {
                            agent_idx: Some(agent_idx),
                            node: RosterBodyNode::Leaf {
                                branch,
                                leaf_idx: body_line.saturating_sub(cursor),
                            },
                        });
                    }
                    cursor = cursor.saturating_add(leaf_len);
                }
            }
        }
    }
    None
}

struct MissionBodyMeta {
    mission_idx: usize,
    agent_row: Option<usize>,
}

fn mission_body_meta(state: &AppState, body_line: usize) -> Option<MissionBodyMeta> {
    let mut cursor = 0usize;
    for (mission_idx, mission) in state.agents.missions.iter().enumerate() {
        let agent_lines = mission.assigned_agents.len().max(1);
        let block_height = agent_lines + 2; // top + bottom border
        if body_line < cursor + block_height {
            let row_in_block = body_line - cursor;
            let agent_row = if row_in_block == 0 || row_in_block + 1 == block_height {
                None
            } else {
                Some(row_in_block.saturating_sub(1))
            };
            return Some(MissionBodyMeta {
                mission_idx,
                agent_row,
            });
        }
        cursor += block_height;
    }
    None
}

struct AlertBodyMeta {
    alert_idx: usize,
    wrap_row: usize,
}

fn alert_body_meta(state: &AppState, width: usize, body_line: usize) -> Option<AlertBodyMeta> {
    let usable = width.max(32);
    let cols_total = usable.saturating_sub(1);
    let widths = allocate_columns(cols_total, &[4, 4, 5, 12], &[5, 8, 10, 26], 3);
    let msg_w = *widths.get(3).unwrap_or(&0);

    let mut cursor = 0usize;
    for (alert_idx, alert) in state.agents.alerts.iter().enumerate() {
        let wrap_rows = wrap_cell_text(&alert.message, msg_w).len().max(1);
        if body_line < cursor + wrap_rows {
            return Some(AlertBodyMeta {
                alert_idx,
                wrap_row: body_line.saturating_sub(cursor),
            });
        }
        cursor += wrap_rows;
    }
    None
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
) {
    let focused = state.focus == PaneId::JobOutput;
    let border_style = if focused {
        Style::default().fg(theme.border_focused)
    } else {
        Style::default().fg(theme.border)
    };
    let border_type = if focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    let title_color = if focused {
        theme.title_focused
    } else {
        theme.title
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            "AGENT OPS",
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(border_style)
        .border_type(border_type)
        .style(Style::default().bg(theme.background));
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);
    if inner.width < 4 || inner.height < 3 {
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tabs
            Constraint::Length(1), // spacer below tabs
            Constraint::Min(1),    // body
            Constraint::Length(1), // footer hints
        ])
        .split(inner);

    render_tab_bar(frame, chunks[0], state, theme);
    render_tab_spacer(frame, chunks[1], theme);

    let rows = current_lines_for_width_with_swarm(state, Some(swarm), chunks[2].width as usize);
    let height = chunks[2].height as usize;
    let max_scroll = rows.len().saturating_sub(height);
    let scroll = state.agents.ops_scroll.min(max_scroll);
    let visible = rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(height)
        .map(|(idx, line)| ops_styled_line(state, idx, line, chunks[2].width as usize, theme))
        .collect::<Vec<_>>();
    let visible = apply_ui_selection(
        visible,
        state.ui_selection.as_ref(),
        UiSelectionPane::JobOutput,
        theme.selection_bg,
        scroll,
    );
    // The roster view uses a table-like presentation; give the whole body area the same base
    // background so the Backends header + roster table feel cohesive.
    let body_bg = match state.agents.dock_tab {
        AgentOpsTab::Roster => ops_table_bg(theme),
        _ => theme.background,
    };
    frame.render_widget(
        Paragraph::new(visible).style(Style::default().bg(body_bg)),
        chunks[2],
    );

    render_footer(frame, chunks[3], state, theme);
}

pub fn render_tab_bar(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let tabs = tab_line(state, theme);
    frame.render_widget(Paragraph::new(tabs), area);
}

fn render_tab_spacer(frame: &mut Frame, area: Rect, theme: &Theme) {
    // Provide breathing room between the tab labels and the body content.
    frame.render_widget(
        Paragraph::new(Line::from("")).style(Style::default().bg(theme.background)),
        area,
    );
}

fn render_footer(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    if area.width < 2 || area.height == 0 {
        return;
    }
    let line = footer_line(state, theme);
    frame.render_widget(Paragraph::new(line), area);
}

fn footer_line(state: &AppState, theme: &Theme) -> Line<'static> {
    let label_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let key_style = Style::default()
        .fg(theme.title)
        .add_modifier(Modifier::BOLD);
    let sep_style = label_style;

    let mut spans: Vec<Span<'static>> = vec![Span::styled("Keys: ", label_style)];

    // Always present: tab/selection/enter semantics.
    spans.push(Span::styled("Tab", key_style));
    spans.push(Span::styled(" tabs", label_style));
    spans.push(Span::styled("  ", sep_style));
    spans.push(Span::styled("j/k", key_style));
    spans.push(Span::styled(" move", label_style));
    spans.push(Span::styled("  ", sep_style));

    match state.agents.dock_tab {
        AgentOpsTab::Roster => {
            spans.push(Span::styled("h/l", key_style));
            spans.push(Span::styled(" size", label_style));
            spans.push(Span::styled("  ", sep_style));
            spans.push(Span::styled("Space", key_style));
            spans.push(Span::styled(" set", label_style));
            spans.push(Span::styled("  ", sep_style));
            spans.push(Span::styled("c", key_style));
            spans.push(Span::styled(" reset", label_style));
            spans.push(Span::styled("  ", sep_style));
            spans.push(Span::styled("Enter", key_style));
            spans.push(Span::styled(" chat", label_style));
        }
        AgentOpsTab::Missions => {
            spans.push(Span::styled("n", key_style));
            spans.push(Span::styled(" new", label_style));
            spans.push(Span::styled("  ", sep_style));
            spans.push(Span::styled("Enter", key_style));
            spans.push(Span::styled(" chat", label_style));
        }
        AgentOpsTab::Evidence => {
            spans.push(Span::styled("Enter", key_style));
            spans.push(Span::styled(" open", label_style));
            spans.push(Span::styled("  ", sep_style));
            spans.push(Span::styled("Esc", key_style));
            spans.push(Span::styled(" close", label_style));
        }
        AgentOpsTab::Mcp => {
            spans.push(Span::styled("r", key_style));
            spans.push(Span::styled(" reconnect", label_style));
            spans.push(Span::styled("  ", sep_style));
            spans.push(Span::styled("s", key_style));
            spans.push(Span::styled(" start", label_style));
            spans.push(Span::styled("  ", sep_style));
            spans.push(Span::styled("x", key_style));
            spans.push(Span::styled(" stop", label_style));
        }
        AgentOpsTab::Alerts => {
            spans.push(Span::styled("Enter", key_style));
            spans.push(Span::styled(" chat", label_style));
        }
        AgentOpsTab::Dag | AgentOpsTab::Diagnostics => {
            spans.push(Span::styled("j/k", key_style));
            spans.push(Span::styled(" scroll", label_style));
        }
        AgentOpsTab::Scratchpad => {
            spans.push(Span::styled("Enter", key_style));
            spans.push(Span::styled(" chat", label_style));
        }
        // Patch is legacy/hidden; render it like Diagnostics.
        AgentOpsTab::Patch => {
            spans.push(Span::styled("j/k", key_style));
            spans.push(Span::styled(" scroll", label_style));
        }
    }

    Line::from(spans)
}

fn tab_line(state: &AppState, theme: &Theme) -> Line<'static> {
    const TABS: [AgentOpsTab; 8] = [
        AgentOpsTab::Roster,
        AgentOpsTab::Missions,
        AgentOpsTab::Dag,
        AgentOpsTab::Evidence,
        AgentOpsTab::Mcp,
        AgentOpsTab::Alerts,
        AgentOpsTab::Diagnostics,
        AgentOpsTab::Scratchpad,
    ];
    const TAB_SPACING: &str = "  ";
    let active = match state.agents.dock_tab {
        AgentOpsTab::Patch => AgentOpsTab::Diagnostics,
        other => other,
    };
    let mut spans = Vec::new();
    for (idx, tab) in TABS.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::raw(TAB_SPACING));
        }
        let style = if active == *tab {
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(theme.border)
        };
        spans.push(Span::styled(tab.label(), style));
    }
    Line::from(spans)
}

pub fn tab_at_column(col: usize) -> Option<AgentOpsTab> {
    const TABS: [AgentOpsTab; 8] = [
        AgentOpsTab::Roster,
        AgentOpsTab::Missions,
        AgentOpsTab::Dag,
        AgentOpsTab::Evidence,
        AgentOpsTab::Mcp,
        AgentOpsTab::Alerts,
        AgentOpsTab::Diagnostics,
        AgentOpsTab::Scratchpad,
    ];
    const TAB_GAP: usize = 2;

    let mut x = 0usize;
    for (idx, tab) in TABS.iter().enumerate() {
        let label = tab.label();
        let end = x.saturating_add(label.len());
        if col >= x && col < end {
            return Some(*tab);
        }
        x = end;
        if idx + 1 < TABS.len() {
            x = x.saturating_add(TAB_GAP);
        }
    }
    None
}

pub fn current_lines(state: &AppState) -> Vec<String> {
    current_lines_for_width_with_swarm(state, None, usize::MAX)
}

pub fn current_lines_for_width(state: &AppState, width: usize) -> Vec<String> {
    current_lines_for_width_with_swarm(state, None, width)
}

pub fn current_lines_for_width_with_swarm(
    state: &AppState,
    swarm: Option<&SwarmRuntime>,
    width: usize,
) -> Vec<String> {
    let usable = width.max(32);
    match state.agents.dock_tab {
        AgentOpsTab::Roster => roster_lines(state, swarm, usable),
        AgentOpsTab::Missions => mission_lines(state, usable),
        AgentOpsTab::Dag => dag_lines(state, swarm, usable),
        AgentOpsTab::Evidence => artifacts_lines(state, swarm, usable),
        AgentOpsTab::Mcp => mcp_lines(state, usable),
        AgentOpsTab::Alerts => alert_lines(state, usable),
        // Patch is hidden from the UI; treat it as Diagnostics for legacy state.
        AgentOpsTab::Patch => diagnostics_lines(state, usable),
        AgentOpsTab::Diagnostics => diagnostics_lines(state, usable),
        AgentOpsTab::Scratchpad => scratchpad_lines(state, usable),
    }
}

pub fn roster_agent_display_order(state: &AppState) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::with_capacity(state.agents.agents.len());
    for (backend, agent_indices) in roster_grouped_agent_indices(state) {
        if !roster_backend_is_expanded(state, backend) {
            continue;
        }
        out.extend(agent_indices);
    }
    out
}

pub fn roster_first_agent_idx_for_backend(
    state: &AppState,
    backend: AgentLaneKind,
) -> Option<usize> {
    roster_grouped_agent_indices(state)
        .into_iter()
        .find(|(kind, _)| *kind == backend)
        .and_then(|(_, agent_indices)| agent_indices.into_iter().next())
}

fn roster_backend_group_for_agent(agent: &nit_core::AgentLane) -> AgentLaneKind {
    if agent.is_codex() {
        AgentLaneKind::Codex
    } else if matches!(agent.kind, AgentLaneKind::Claude) {
        AgentLaneKind::Claude
    } else if matches!(agent.kind, AgentLaneKind::Gemini) {
        AgentLaneKind::Gemini
    } else if matches!(agent.kind, AgentLaneKind::Mock) {
        AgentLaneKind::Mock
    } else {
        AgentLaneKind::Unknown
    }
}

fn roster_backend_group_label(kind: AgentLaneKind) -> &'static str {
    match kind {
        AgentLaneKind::Codex => "Codex",
        AgentLaneKind::Claude => "Claude",
        AgentLaneKind::Gemini => "Gemini",
        AgentLaneKind::Mock => "Local",
        AgentLaneKind::Unknown => "Other",
    }
}

fn roster_grouped_agent_indices(state: &AppState) -> Vec<(AgentLaneKind, Vec<usize>)> {
    let mut codex: Vec<usize> = Vec::new();
    let mut claude: Vec<usize> = Vec::new();
    let mut gemini: Vec<usize> = Vec::new();
    let mut local: Vec<usize> = Vec::new();
    let mut other: Vec<usize> = Vec::new();

    for (idx, agent) in state.agents.agents.iter().enumerate() {
        if agent.shadow {
            continue;
        }
        match roster_backend_group_for_agent(agent) {
            AgentLaneKind::Codex => codex.push(idx),
            AgentLaneKind::Claude => claude.push(idx),
            AgentLaneKind::Gemini => gemini.push(idx),
            AgentLaneKind::Mock => local.push(idx),
            AgentLaneKind::Unknown => other.push(idx),
        }
    }

    let mut out: Vec<(AgentLaneKind, Vec<usize>)> = Vec::with_capacity(5);
    if !codex.is_empty() {
        out.push((AgentLaneKind::Codex, codex));
    }
    if !claude.is_empty() {
        out.push((AgentLaneKind::Claude, claude));
    }
    if !gemini.is_empty() {
        out.push((AgentLaneKind::Gemini, gemini));
    }
    if !local.is_empty() {
        out.push((AgentLaneKind::Mock, local));
    }
    if !other.is_empty() {
        out.push((AgentLaneKind::Unknown, other));
    }
    out
}

fn roster_backend_is_expanded(state: &AppState, backend: AgentLaneKind) -> bool {
    state
        .agents
        .roster_expanded_backend_kinds
        .contains(&backend)
}

fn roster_lines(state: &AppState, swarm: Option<&SwarmRuntime>, width: usize) -> Vec<String> {
    let widths = roster_column_widths(width);
    let selected_row = roster_selected_row(state);
    let mut out = vec![" Backends".into()];
    for row in roster_backend_inventory_rows(state) {
        match row.backend {
            BackendInventoryBackend::Codex => out.push(roster_backend_line(
                "Codex",
                if row.available {
                    "available"
                } else {
                    "not found"
                },
                if row.active { "active" } else { "idle" },
            )),
            BackendInventoryBackend::Claude => out.push(roster_backend_line(
                "Claude",
                if row.available {
                    "available"
                } else {
                    "not found"
                },
                if row.active { "active" } else { "idle" },
            )),
            BackendInventoryBackend::Gemini => out.push(roster_backend_line(
                "Gemini",
                if row.available {
                    "available"
                } else {
                    "not found"
                },
                if row.active { "active" } else { "idle" },
            )),
            BackendInventoryBackend::Local => out.push(roster_backend_line(
                "Local",
                "built-in",
                if row.active { "active" } else { "idle" },
            )),
        }
    }

    out.push(String::new());
    out.push(ROSTER_SWARM_TEMPLATE_LINE.into());
    out.push(ROSTER_SWARM_MISSION_LINE.into());
    out.push(String::new());
    out.push(format!(
        " {} {} {} {} {}",
        fit_left("PRI+ROLE", widths[0]),
        fit_left("STATUS", widths[1]),
        fit_right("HB", widths[2]),
        fit_right("Q", widths[3]),
        fit_left("MISSION", widths[4]),
    ));
    out.push("─".repeat(width.min(240)));

    let show_roles = matches!(
        state
            .agents
            .swarm_default_template
            .to_ascii_lowercase()
            .as_str(),
        "bulk" | "parallel"
    );
    if state.agents.agents.is_empty() {
        out.push(" No agents available.".into());
        out.push("─".repeat(width.min(240)));
        return out;
    }

    let mut swarm_dash_by_mission_id: std::collections::HashMap<
        String,
        Option<SwarmDashboardView>,
    > = std::collections::HashMap::new();
    for (backend, agent_indices) in roster_grouped_agent_indices(state) {
        let backend_selected = selected_row == Some(RosterSelectableRow::Backend { backend });
        let label = format!(
            "{} {}",
            if roster_backend_is_expanded(state, backend) {
                tree_open_glyph()
            } else {
                tree_closed_glyph()
            },
            roster_backend_group_label(backend)
        );
        out.push(format!(
            "{}{} {} {} {} {}",
            if backend_selected {
                cursor_glyph()
            } else {
                ' '
            },
            fit_left(&label, widths[0]),
            fit_left("", widths[1]),
            fit_right("", widths[2]),
            fit_right("", widths[3]),
            fit_left("", widths[4]),
        ));
        if !roster_backend_is_expanded(state, backend) {
            continue;
        }

        for agent_idx in agent_indices {
            let Some(agent) = state.agents.agents.get(agent_idx) else {
                continue;
            };

            let is_clone = is_swarm_clone_agent_id(agent.id.as_str())
                || is_chat_clone_agent_id(agent.id.as_str());
            let agent_selected = selected_row == Some(RosterSelectableRow::Agent { agent_idx });
            let priority_prefix = if agent.supports_swarm_priority() && !is_clone {
                if state.agents.swarm_priority_agent_ids.contains(&agent.id) {
                    "[x] "
                } else {
                    "[ ] "
                }
            } else {
                "    "
            };
            let marker = if agent_selected && state.agents.roster_tree_selected.is_none() {
                cursor_glyph()
            } else {
                arrow_glyph()
            };
            let role_label = if is_swarm_clone_agent_id(agent.id.as_str()) {
                let assigned_role = (|| {
                    let swarm = swarm?;
                    let (mission_id, _suffix) = swarm_clone_label_parts(agent.id.as_str())?;
                    let dashboard = swarm_dash_by_mission_id
                        .entry(mission_id.to_string())
                        .or_insert_with(|| swarm.swarm_dashboard(mission_id));
                    let dashboard = dashboard.as_ref()?;
                    swarm_assigned_roles_for_agent(dashboard, agent.id.as_str())
                })();
                let label = swarm_clone_display_label(agent.id.as_str(), assigned_role.as_deref())
                    .unwrap_or_else(|| "swarm clone".into());
                format!("    {} {label}", arrow_glyph())
            } else if is_chat_clone_agent_id(agent.id.as_str()) {
                let label = table_role_label(agent.role.as_str());
                format!("    {} {label}", arrow_glyph())
            } else {
                format!("{priority_prefix}{}", table_role_label(agent.role.as_str()))
            };
            out.push(format!(
                "{marker}{} {} {} {} {}",
                fit_left(&role_label, widths[0]),
                fit_left(agent.status.label(), widths[1]),
                fit_right(&format!("{}s", agent.heartbeat_age_secs), widths[2]),
                fit_right(&agent.queue_len.to_string(), widths[3]),
                fit_left(agent.current_mission.as_deref().unwrap_or("--"), widths[4]),
            ));

            // Expand the selected model into a small tree: Size (Codex reasoning effort levels) and
            // Role (swarm planning hints).
            if agent_idx == state.agents.roster_selected
                && !is_clone
                && !state
                    .agents
                    .roster_tree_collapsed_agent_ids
                    .contains(&agent.id)
            {
                let efforts = state
                    .agents
                    .codex_supported_reasoning_efforts
                    .get(&agent.id)
                    .or_else(|| state.agents.claude_supported_efforts.get(&agent.id))
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let has_size = !efforts.is_empty();
                let has_roles = show_roles && (agent.is_codex() || agent.is_claude());
                if !has_size && !has_roles {
                    continue;
                }

                if has_size {
                    let label = format!("    {} Size", arrow_glyph());
                    out.push(format!(
                        " {} {} {} {} {}",
                        fit_left(&label, widths[0]),
                        fit_left("", widths[1]),
                        fit_right("", widths[2]),
                        fit_right("", widths[3]),
                        fit_left("", widths[4]),
                    ));

                    let chosen = state
                        .agents
                        .codex_selected_reasoning_effort
                        .get(&agent.id)
                        .or_else(|| state.agents.codex_default_reasoning_effort.get(&agent.id))
                        .or_else(|| state.agents.claude_selected_effort.get(&agent.id))
                        .or_else(|| state.agents.claude_default_effort.get(&agent.id))
                        .map(|s| s.as_str());
                    for (effort_idx, effort) in efforts.iter().enumerate() {
                        let marker = if state.agents.roster_tree_selected
                            == Some(RosterTreeSelection {
                                branch: RosterTreeBranch::Size,
                                leaf_idx: effort_idx,
                            }) {
                            cursor_glyph()
                        } else {
                            ' '
                        };
                        let checked = if chosen == Some(effort.as_str()) {
                            "x"
                        } else {
                            " "
                        };
                        let label = format!("      {} [{checked}] {effort}", arrow_glyph());
                        out.push(format!(
                            "{marker}{} {} {} {} {}",
                            fit_left(&label, widths[0]),
                            fit_left("", widths[1]),
                            fit_right("", widths[2]),
                            fit_right("", widths[3]),
                            fit_left("", widths[4]),
                        ));
                    }
                }

                if has_roles {
                    let label = format!("    {} Role", arrow_glyph());
                    out.push(format!(
                        " {} {} {} {} {}",
                        fit_left(&label, widths[0]),
                        fit_left("", widths[1]),
                        fit_right("", widths[2]),
                        fit_right("", widths[3]),
                        fit_left("", widths[4]),
                    ));

                    let chosen = state
                        .agents
                        .swarm_role_by_agent_id
                        .get(&agent.id)
                        .map(|s| s.trim().to_ascii_lowercase())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "all".into());
                    let chosen = normalize_roster_role_hint(chosen.as_str());
                    for (role_idx, role) in ROSTER_ROLE_OPTIONS.iter().enumerate() {
                        let marker = if state.agents.roster_tree_selected
                            == Some(RosterTreeSelection {
                                branch: RosterTreeBranch::Role,
                                leaf_idx: role_idx,
                            }) {
                            cursor_glyph()
                        } else {
                            ' '
                        };
                        let checked = if chosen == normalize_roster_role_hint(role) {
                            "x"
                        } else {
                            " "
                        };
                        let display = if *role == "all" { "All" } else { role };
                        let label = format!("      {} [{checked}] {display}", arrow_glyph());
                        out.push(format!(
                            "{marker}{} {} {} {} {}",
                            fit_left(&label, widths[0]),
                            fit_left("", widths[1]),
                            fit_right("", widths[2]),
                            fit_right("", widths[3]),
                            fit_left("", widths[4]),
                        ));
                    }
                }
            }
        }
    }
    out.push("─".repeat(width.min(240)));
    out
}

fn mission_lines(state: &AppState, width: usize) -> Vec<String> {
    let cols_total = width.saturating_sub(1);
    // Put AGENTS last so it soaks up extra width.
    let widths = allocate_columns(cols_total, &[6, 6, 3, 6, 8], &[12, 8, 5, 12, 18], 4);
    let mut out = vec![
        format!(
            " {} {} {} {} {}",
            fit_left("MISSION", widths[0]),
            fit_left("PHASE", widths[1]),
            fit_left("SWM", widths[2]),
            fit_left("STATUS", widths[3]),
            fit_left("AGENTS", widths[4]),
        ),
        "─".repeat(width.min(240)),
    ];
    if state.agents.missions.is_empty() {
        out.push(" No missions yet. Press n to create one.".into());
        return out;
    }
    for (idx, mission) in state.agents.missions.iter().enumerate() {
        let empty0 = fit_left("", widths[0]);
        let empty1 = fit_left("", widths[1]);
        let empty2 = fit_left("", widths[2]);
        let empty3 = fit_left("", widths[3]);

        out.push(format!(
            " {} {} {} {} {}",
            empty0,
            empty1,
            empty2,
            empty3,
            agent_pane_top(widths[4]),
        ));
        let agent_lines = mission.assigned_agents.len().max(1);
        for agent_idx in 0..agent_lines {
            let marker = if idx == state.agents.mission_selected {
                if agent_idx == 0 {
                    cursor_glyph()
                } else {
                    arrow_glyph()
                }
            } else {
                arrow_glyph()
            };
            let agent_label = mission
                .assigned_agents
                .get(agent_idx)
                .map(|s| swarm_clone_display_label(s.as_str(), None).unwrap_or_else(|| s.clone()))
                .unwrap_or_else(|| "--".into());
            if agent_idx == 0 {
                out.push(format!(
                    "{marker}{} {} {} {} {}",
                    fit_left(&mission.id, widths[0]),
                    fit_left(mission.phase.label(), widths[1]),
                    fit_left(if mission.swarm { "yes" } else { "no" }, widths[2]),
                    fit_left(&mission.status, widths[3]),
                    agent_pane_mid(&agent_label, widths[4]),
                ));
            } else {
                out.push(format!(
                    "{marker}{} {} {} {} {}",
                    empty0,
                    empty1,
                    empty2,
                    empty3,
                    agent_pane_mid(&agent_label, widths[4]),
                ));
            }
        }
        out.push(format!(
            " {} {} {} {} {}",
            empty0,
            empty1,
            empty2,
            empty3,
            agent_pane_bottom(widths[4]),
        ));
    }
    out
}

fn roster_backend_line(name: &str, primary: &str, secondary: &str) -> String {
    format!("  {name:<ROSTER_BACKEND_NAME_W$}  {primary}  {secondary}")
}

fn agent_pane_inner_width(col_width: usize) -> usize {
    col_width.saturating_sub(2)
}

fn agent_pane_top(col_width: usize) -> String {
    if col_width == 0 {
        return String::new();
    }
    if col_width == 1 {
        return "│".into();
    }
    let inner = agent_pane_inner_width(col_width);
    let mut out = String::with_capacity(col_width);
    out.push('┌');
    out.push_str(&"─".repeat(inner));
    out.push('┐');
    out
}

fn agent_pane_bottom(col_width: usize) -> String {
    if col_width == 0 {
        return String::new();
    }
    if col_width == 1 {
        return "│".into();
    }
    let inner = agent_pane_inner_width(col_width);
    let mut out = String::with_capacity(col_width);
    out.push('└');
    out.push_str(&"─".repeat(inner));
    out.push('┘');
    out
}

fn agent_pane_mid(text: &str, col_width: usize) -> String {
    if col_width == 0 {
        return String::new();
    }
    if col_width == 1 {
        return "│".into();
    }
    let inner = agent_pane_inner_width(col_width);
    let mut out = String::with_capacity(col_width);
    out.push('│');
    out.push_str(&fit_left(text, inner));
    out.push('│');
    out
}

fn mcp_lines(state: &AppState, width: usize) -> Vec<String> {
    let mcp = &state.agents.mcp;
    let key_w = 11usize.min(width.saturating_sub(4));
    // One leading spacer + one spacer between KEY and VALUE.
    let value_w = width.saturating_sub(key_w + 2).max(1);
    let latency = mcp
        .latency_ms
        .map(|ms| format!("{ms}ms"))
        .unwrap_or_else(|| "--".into());
    let mut out = vec![
        format!(" {} {}", fit_left("KEY", key_w), fit_left("VALUE", value_w)),
        "─".repeat(width.min(240)),
    ];
    push_wrapped_kv(
        &mut out,
        key_w,
        value_w,
        "STATE",
        mcp_state_label(mcp.state),
        false,
    );
    push_wrapped_kv(&mut out, key_w, value_w, "ENDPOINT", &mcp.endpoint, false);
    push_wrapped_kv(&mut out, key_w, value_w, "LATENCY", &latency, false);
    if let Some(err) = mcp.last_error.as_deref() {
        let err = format_mcp_error_for_display(err);
        push_wrapped_kv(&mut out, key_w, value_w, "LAST ERR", &err, true);
    }
    out.push(String::new());
    out.push(" [r] reconnect   [s] start   [x] stop".into());
    out
}

fn push_wrapped_kv(
    out: &mut Vec<String>,
    key_w: usize,
    value_w: usize,
    key: &str,
    value: &str,
    repeat_key: bool,
) {
    let chunks = wrap_cell_text(value, value_w);
    for (idx, chunk) in chunks.iter().enumerate() {
        let key_cell = if idx == 0 || repeat_key { key } else { "" };
        out.push(format!(
            " {} {}",
            fit_left(key_cell, key_w),
            fit_left(chunk, value_w)
        ));
    }
}

fn format_mcp_error_for_display(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Ok(pretty) = serde_json::to_string_pretty(&json) {
            return pretty;
        }
    }
    trimmed.to_string()
}

fn mcp_state_label(state: McpConnectionState) -> &'static str {
    state.label()
}

fn alert_lines(state: &AppState, width: usize) -> Vec<String> {
    let cols_total = width.saturating_sub(1);
    let widths = allocate_columns(cols_total, &[4, 4, 5, 12], &[5, 8, 10, 26], 3);
    let mut out = vec![
        format!(
            " {} {} {} {}",
            fit_left("SEV", widths[0]),
            fit_left("TIME", widths[1]),
            fit_left("SOURCE", widths[2]),
            fit_left("MESSAGE", widths[3]),
        ),
        "─".repeat(width.min(240)),
    ];
    if state.agents.alerts.is_empty() {
        out.push(" No alerts.".into());
        return out;
    }
    for (idx, alert) in state.agents.alerts.iter().enumerate() {
        let sev = alert_severity_label(alert.severity);
        let chunks = wrap_cell_text(&alert.message, widths[3]);
        for (row, chunk) in chunks.iter().enumerate() {
            let selected = idx == state.agents.alert_selected;
            let marker = if row == 0 && selected {
                cursor_glyph()
            } else if row == 0 {
                arrow_glyph()
            } else {
                ' '
            };
            let (sev_cell, time_cell, src_cell) = if row == 0 {
                (sev, alert.at.as_str(), alert.source.as_str())
            } else {
                ("", "", "")
            };
            out.push(format!(
                "{marker}{} {} {} {}",
                fit_left(sev_cell, widths[0]),
                fit_left(time_cell, widths[1]),
                fit_left(src_cell, widths[2]),
                fit_left(chunk, widths[3]),
            ));
        }
    }
    out
}

fn dag_lines(state: &AppState, swarm: Option<&SwarmRuntime>, width: usize) -> Vec<String> {
    let width = width.max(32);
    let mut out = vec![" DAG".into(), "─".repeat(width.min(240))];
    let Some(mission_id) = state.agents.selected_context_mission() else {
        out.push(" No mission selected.".into());
        return out;
    };
    if let Some(mission) = state
        .agents
        .missions
        .iter()
        .find(|mission| mission.id == mission_id)
    {
        if !mission.swarm {
            out.push(format!(" Mission {mission_id} is not a swarm."));
            return out;
        }
    }
    let Some(swarm) = swarm else {
        out.push(" Swarm runtime unavailable.".into());
        return out;
    };
    let Some(dashboard) = swarm.swarm_dashboard(mission_id) else {
        out.push(" No DAG data for this mission yet.".into());
        return out;
    };
    dag_lines_for_dashboard(&dashboard, width)
}

fn dag_task_widths(cols_total: usize) -> Vec<usize> {
    allocate_columns(cols_total, &[9, 7, 10], &[10, 7, 24], 2)
}

fn dag_gate_widths(cols_total: usize) -> Vec<usize> {
    allocate_columns(cols_total, &[4, 6, 10], &[10, 8, 24], 2)
}

fn dag_kv_block_lines(pairs: &[(&str, String)], width: usize) -> Vec<String> {
    let width = width.max(32);
    let indent = " ";
    let indent_len = indent.chars().count();
    let sep = " | ";
    let sep_len = sep.chars().count();
    let max_seg_len = width.saturating_sub(indent_len);

    let mut segments: Vec<String> = Vec::new();
    for (label, value) in pairs {
        let label = label.trim();
        let value = value.trim();
        let segment = format!("{label}: {value}");
        if segment.chars().count() <= max_seg_len {
            segments.push(segment);
            continue;
        }

        let prefix = format!("{label}: ");
        let avail = max_seg_len.saturating_sub(prefix.chars().count());
        if avail == 0 {
            segments.push(format!("{label}:"));
            continue;
        }
        for chunk in wrap_cell_text(value, avail) {
            segments.push(format!("{label}: {chunk}"));
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_len = indent_len;

    for segment in segments {
        let seg_len = segment.chars().count();
        if current.is_empty() {
            current.push(segment);
            current_len = indent_len.saturating_add(seg_len);
            continue;
        }

        if current_len.saturating_add(sep_len).saturating_add(seg_len) <= width {
            current.push(segment);
            current_len = current_len.saturating_add(sep_len).saturating_add(seg_len);
            continue;
        }

        out.push(format!("{indent}{}", current.join(sep)));
        current.clear();
        current.push(segment);
        current_len = indent_len.saturating_add(seg_len);
    }

    if !current.is_empty() {
        out.push(format!("{indent}{}", current.join(sep)));
    }
    if out.is_empty() {
        out.push(indent.to_string());
    }
    out
}

fn dag_lines_for_dashboard(dashboard: &SwarmDashboardView, width: usize) -> Vec<String> {
    let width = width.max(32);
    let cols_total = width.saturating_sub(1);
    let total = dashboard.tasks.len();
    let pending_work = dashboard.pending > 0 && dashboard.running == 0 && dashboard.queued == 0;
    let status_word = match dashboard.phase.as_str() {
        "PLAN" => "PLAN",
        "VERIFY" => "VERIFY",
        "SYNTH" => "SYNTH",
        _ => {
            if total == 0 {
                "EMPTY"
            } else if dashboard.failed > 0 {
                "FAILED"
            } else if dashboard.running > 0 {
                "RUNNING"
            } else if dashboard.queued > 0 {
                "QUEUED"
            } else if pending_work {
                "PENDING"
            } else if dashboard.done + dashboard.skipped == total {
                "DONE"
            } else {
                "IDLE"
            }
        }
    };

    let mut out = vec![" DAG".into(), "─".repeat(width.min(240))];
    out.extend(dag_kv_block_lines(
        &[
            ("Status", status_word.to_string()),
            ("Mission", dashboard.mission_id.clone()),
            ("Template", dashboard.template.clone()),
            ("Phase", dashboard.phase.clone()),
        ],
        width,
    ));

    out.extend(dag_kv_block_lines(
        &[
            ("Done", format!("{}/{}", dashboard.done, total)),
            ("Fail", dashboard.failed.to_string()),
            ("Run", dashboard.running.to_string()),
            ("Queue", dashboard.queued.to_string()),
            ("Pending", dashboard.pending.to_string()),
            ("Skip", dashboard.skipped.to_string()),
        ],
        width,
    ));

    if let Some(bundle) = dashboard.gate_bundle.as_deref() {
        out.extend(dag_kv_block_lines(&[("Gate", bundle.to_string())], width));
    }

    let task_widths = dag_task_widths(cols_total);
    let empty_id = fit_left("", task_widths[0]);
    let empty_state = fit_left("", task_widths[1]);
    out.push(format!(
        " {} {} {}",
        fit_left("ID", task_widths[0]),
        fit_left("STATE", task_widths[1]),
        fit_left("TITLE", task_widths[2]),
    ));
    out.push("─".repeat(width.min(240)));

    if dashboard.tasks.is_empty() {
        if dashboard.phase == "PLAN" {
            let line = "Planning: waiting for planner output";
            out.push(format!(
                " {} {} {}",
                empty_id,
                empty_state,
                fit_left(line, task_widths[2]),
            ));
        } else {
            out.push(format!(
                " {} {} {}",
                empty_id,
                empty_state,
                fit_left("No tasks.", task_widths[2]),
            ));
        }
    } else {
        for task in dashboard.tasks.iter() {
            let blocked = if task.blocked_on.is_empty() {
                "-".to_string()
            } else {
                task.blocked_on.join(",")
            };
            let deps = if task.deps.is_empty() {
                "-".to_string()
            } else {
                task.deps.join(",")
            };
            let role = task
                .role
                .as_deref()
                .map(table_role_label)
                .unwrap_or_else(|| "-".into());
            let writes = if task.writes { "yes" } else { "no" };
            let out_present = if task.output_present { "yes" } else { "no" };
            let done_when = task
                .done_when
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("-");

            let title_chunks = wrap_cell_text(task.title.as_str(), task_widths[2]);
            for (idx, chunk) in title_chunks.iter().enumerate() {
                let (id_cell, state_cell) = if idx == 0 {
                    (task.id.as_str(), task.state.as_str())
                } else {
                    ("", "")
                };
                let marker = if idx == 0 { arrow_glyph() } else { ' ' };
                out.push(format!(
                    "{marker} {} {} {}",
                    fit_left(id_cell, task_widths[0]),
                    fit_left(state_cell, task_widths[1]),
                    fit_left(chunk, task_widths[2]),
                ));
            }

            let details_1 = format!("agent: {}  role: {role}", task.agent_id);
            let details_1_chunks = wrap_cell_text(&details_1, task_widths[2].saturating_sub(2));
            for chunk in details_1_chunks {
                let line = format!("{} {chunk}", arrow_glyph());
                out.push(format!(
                    " {} {} {}",
                    empty_id,
                    empty_state,
                    fit_left(&line, task_widths[2]),
                ));
            }

            let details_2 = format!(
                "deps: {deps}  block: {blocked}  writes: {writes}  out: {out_present}  \
done_when: {done_when}"
            );
            let details_2_chunks = wrap_cell_text(&details_2, task_widths[2].saturating_sub(2));
            for chunk in details_2_chunks {
                let line = format!("{} {chunk}", arrow_glyph());
                out.push(format!(
                    " {} {} {}",
                    empty_id,
                    empty_state,
                    fit_left(&line, task_widths[2]),
                ));
            }
        }
    }

    if !dashboard.gates.is_empty() {
        out.push("─".repeat(width.min(240)));
        let gate_widths = dag_gate_widths(cols_total);
        out.push(format!(
            " {} {} {}",
            fit_left("GATE", gate_widths[0]),
            fit_left("STATUS", gate_widths[1]),
            fit_left("COMMAND", gate_widths[2]),
        ));
        for gate in dashboard.gates.iter() {
            // Always render the raw command first so the user can see/copy it even when notes are
            // long. Notes are rendered on separate continuation lines.
            let cmd_width = gate_widths[2];
            let wrap_width = if cmd_width > 2 {
                cmd_width.saturating_sub(2)
            } else {
                cmd_width
            };
            let mut chunks = wrap_cell_text(gate.command.as_str(), wrap_width);
            if chunks.len() > 1 {
                let cont_len = chunks.len().saturating_sub(1);
                for chunk in chunks.iter_mut().take(cont_len) {
                    if cmd_width > 2 {
                        chunk.push_str(" \\");
                    } else {
                        chunk.push('\\');
                    }
                }
            }
            for (idx, chunk) in chunks.iter().enumerate() {
                let gate_cell = if idx == 0 { gate.name.as_str() } else { "" };
                out.push(format!(
                    " {} {} {}",
                    fit_left(gate_cell, gate_widths[0]),
                    fit_left(gate.status.as_str(), gate_widths[1]),
                    fit_left(chunk, gate_widths[2]),
                ));
            }

            let Some(notes) = gate
                .notes
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };

            let note_chunks = wrap_cell_text(notes, gate_widths[2].saturating_sub(2));
            for chunk in note_chunks {
                let line = format!("{} {chunk}", arrow_glyph());
                out.push(format!(
                    " {} {} {}",
                    fit_left("", gate_widths[0]),
                    fit_left(gate.status.as_str(), gate_widths[1]),
                    fit_left(&line, gate_widths[2]),
                ));
            }
        }
    }

    out
}

fn task_artifacts_present(artifacts: &SwarmTaskArtifacts) -> bool {
    artifacts
        .summary
        .as_deref()
        .is_some_and(|summary| !summary.trim().is_empty())
        || !artifacts.files.is_empty()
        || !artifacts.diffs.is_empty()
        || !artifacts.commands.is_empty()
        || !artifacts.risks.is_empty()
        || !artifacts.notes.is_empty()
}

fn sanitize_artifact_path_segment(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn summarize_items(items: Vec<String>, limit: usize) -> String {
    let total = items.len();
    let shown = items.into_iter().take(limit).collect::<Vec<_>>();
    if shown.is_empty() {
        return String::new();
    }
    if total > shown.len() {
        format!("{}; +{} more", shown.join("; "), total - shown.len())
    } else {
        shown.join("; ")
    }
}

fn gate_report_status_label(gate: &GateReportGate) -> &'static str {
    if let Some(status) = gate.status.as_deref() {
        if status.eq_ignore_ascii_case("pass")
            || status.eq_ignore_ascii_case("ok")
            || status.eq_ignore_ascii_case("success")
        {
            return "PASS";
        }
        if status.eq_ignore_ascii_case("skip") || status.eq_ignore_ascii_case("skipped") {
            return "SKIP";
        }
        if status.eq_ignore_ascii_case("fail") || status.eq_ignore_ascii_case("failed") {
            return "FAIL";
        }
    }
    if gate.ok {
        "PASS"
    } else {
        "FAIL"
    }
}

fn push_wrapped_detail(out: &mut Vec<String>, label: &str, value: &str, width: usize) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    let prefix = format!(" {}: ", label.trim());
    let available = width.saturating_sub(prefix.chars().count()).max(8);
    for chunk in wrap_cell_text(value, available) {
        out.push(format!("{prefix}{chunk}"));
    }
}

fn summarize_text_preview(text: &str, max_chars: usize) -> String {
    let compact = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
    if compact.chars().count() <= max_chars {
        compact
    } else {
        let head = compact.chars().take(max_chars).collect::<String>();
        format!("{head}...")
    }
}

#[derive(Clone, Debug)]
enum ArtifactRef {
    Message {
        idx: usize,
    },
    Patch {
        idx: usize,
    },
    Evidence {
        idx: usize,
    },
    PersistedMessage {
        message: AgentMessage,
    },
    PersistedPatch {
        patch: PersistedPatchRecord,
        path: Option<String>,
    },
    PersistedEvidence {
        item: EvidenceItem,
    },
    SwarmTask {
        mission_id: String,
        task_id: String,
    },
    SwarmReport {
        mission_id: String,
    },
    SwarmVerify {
        mission_id: String,
    },
}

#[derive(Clone, Debug)]
pub struct ArtifactCard {
    pub kind: &'static str,
    pub at: String,
    pub owner: String,
    pub preview: String,
    reference: ArtifactRef,
}

const ARTIFACT_CARD_LIMIT_REPLIES: usize = 48;
const ARTIFACT_CARD_LIMIT_PATCHES: usize = 24;
const ARTIFACT_CARD_LIMIT_EVIDENCE: usize = 24;

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct PersistedPatchRecord {
    pub id: String,
    #[serde(default)]
    pub mission_id: Option<String>,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub diff: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
struct PersistedArtifactsRun {
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    messages: Vec<AgentMessage>,
    #[serde(default)]
    patches: Vec<PersistedPatchRecord>,
    #[serde(default)]
    evidence: Vec<EvidenceItem>,
    #[serde(default)]
    codex_thread_id: Option<String>,
    #[serde(default)]
    codex_thread_ids: Option<BTreeMap<String, String>>,
    #[serde(default)]
    swarm: bool,
    #[serde(default)]
    assigned_agents: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SavedArtifactsRunKind {
    Current,
    Archived,
}

#[derive(Clone, Debug)]
pub struct SavedArtifactsRunEntry {
    pub kind: SavedArtifactsRunKind,
    pub label: String,
    pub detail: String,
    pub run_path: Option<String>,
    pub archive_micros: Option<u128>,
}

fn persisted_mission_run_path(state: &AppState, mission_id: &str) -> PathBuf {
    state
        .workspace_root
        .join(".nit")
        .join("agents")
        .join("runs")
        .join(mission_id)
        .join("run.json")
}

fn persisted_mission_history_root(state: &AppState, mission_id: &str) -> PathBuf {
    state
        .workspace_root
        .join(".nit")
        .join("agents")
        .join("runs")
        .join(mission_id)
        .join("history")
}

fn persisted_ad_hoc_run_path(state: &AppState, agent_id: &str) -> PathBuf {
    state
        .workspace_root
        .join(".nit")
        .join("agents")
        .join("ad-hoc")
        .join(sanitize_artifact_path_segment(agent_id))
        .join("run.json")
}

fn persisted_ad_hoc_history_root(state: &AppState, agent_id: &str) -> PathBuf {
    state
        .workspace_root
        .join(".nit")
        .join("agents")
        .join("ad-hoc")
        .join(sanitize_artifact_path_segment(agent_id))
        .join("history")
}

fn load_persisted_artifacts_run(path: PathBuf) -> Option<PersistedArtifactsRun> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn load_persisted_mission_run(state: &AppState, mission_id: &str) -> Option<PersistedArtifactsRun> {
    load_persisted_artifacts_run(persisted_mission_run_path(state, mission_id))
}

fn load_persisted_ad_hoc_run(state: &AppState, agent_id: &str) -> Option<PersistedArtifactsRun> {
    load_persisted_artifacts_run(persisted_ad_hoc_run_path(state, agent_id))
}

fn persisted_ad_hoc_root(agent_id: &str) -> String {
    format!(
        ".nit/agents/ad-hoc/{}/",
        sanitize_artifact_path_segment(agent_id)
    )
}

fn persisted_run_updated_label(run: &PersistedArtifactsRun) -> String {
    run.updated_at
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("--")
        .to_string()
}

fn persisted_history_archive_micros(path: &Path) -> Option<u128> {
    let file_name = path.file_name()?.to_string_lossy();
    let prefix = file_name
        .split_once('-')
        .map(|(prefix, _)| prefix)
        .unwrap_or(file_name.as_ref());
    Some(prefix)
        .filter(|value| !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit()))
        .and_then(|value| value.parse::<u128>().ok())
}

fn format_saved_run_relative_label_from_micros(
    archive_micros: Option<u128>,
    now_micros: u128,
) -> String {
    let Some(archive_micros) = archive_micros else {
        return "saved run".into();
    };
    let elapsed = now_micros.saturating_sub(archive_micros) / 1_000_000;
    if elapsed < 30 {
        return "saved just now".into();
    }
    if elapsed < 60 * 60 {
        return format!("saved {}m ago", elapsed / 60);
    }
    if elapsed < 24 * 60 * 60 {
        return format!("saved {}h ago", elapsed / (60 * 60));
    }
    if elapsed < 30 * 24 * 60 * 60 {
        return format!("saved {}d ago", elapsed / (24 * 60 * 60));
    }
    if elapsed < 365 * 24 * 60 * 60 {
        return format!("saved {}mo ago", elapsed / (30 * 24 * 60 * 60));
    }
    format!("saved {}y ago", elapsed / (365 * 24 * 60 * 60))
}

fn format_saved_run_absolute_label_from_micros(archive_micros: Option<u128>) -> Option<String> {
    let nanos = archive_micros?.checked_mul(1_000)?;
    let nanos = i128::try_from(nanos).ok()?;
    let datetime = OffsetDateTime::from_unix_timestamp_nanos(nanos).ok()?;
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02} UTC",
        datetime.year(),
        datetime.month() as u8,
        datetime.day(),
        datetime.hour(),
        datetime.minute()
    ))
}

fn saved_run_detail_label(archive_micros: Option<u128>, run_updated: &str, counts: &str) -> String {
    let absolute = format_saved_run_absolute_label_from_micros(archive_micros);
    match (
        absolute.as_deref().filter(|value| !value.is_empty()),
        run_updated.trim(),
    ) {
        (Some(absolute), run_updated) if !run_updated.is_empty() && run_updated != "--" => {
            format!("{absolute} · run {run_updated} · {counts}")
        }
        (Some(absolute), _) => format!("{absolute} · {counts}"),
        (None, run_updated) if !run_updated.is_empty() && run_updated != "--" => {
            format!("{run_updated} · {counts}")
        }
        (None, _) => counts.to_string(),
    }
}

pub fn saved_run_history_filter_label(filter: SavedRunHistoryFilter) -> &'static str {
    match filter {
        SavedRunHistoryFilter::All => "all",
        SavedRunHistoryFilter::LastDay => "24h",
        SavedRunHistoryFilter::LastWeek => "7d",
        SavedRunHistoryFilter::LastMonth => "30d",
    }
}

fn saved_run_history_filter_window_micros(filter: SavedRunHistoryFilter) -> Option<u128> {
    const DAY_MICROS: u128 = 24 * 60 * 60 * 1_000_000;
    match filter {
        SavedRunHistoryFilter::All => None,
        SavedRunHistoryFilter::LastDay => Some(DAY_MICROS),
        SavedRunHistoryFilter::LastWeek => Some(7 * DAY_MICROS),
        SavedRunHistoryFilter::LastMonth => Some(30 * DAY_MICROS),
    }
}

fn saved_run_entry_matches_filter(
    entry: &SavedArtifactsRunEntry,
    filter: SavedRunHistoryFilter,
    now_micros: u128,
) -> bool {
    if matches!(entry.kind, SavedArtifactsRunKind::Current) {
        return true;
    }
    let Some(window_micros) = saved_run_history_filter_window_micros(filter) else {
        return true;
    };
    let Some(archive_micros) = entry.archive_micros else {
        return true;
    };
    now_micros.saturating_sub(archive_micros) <= window_micros
}

fn history_entries_for_root<F>(history_root: PathBuf, make_detail: F) -> Vec<SavedArtifactsRunEntry>
where
    F: Fn(&PersistedArtifactsRun) -> String,
{
    let mut entries = Vec::new();
    let now_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    let read_dir = match fs::read_dir(&history_root) {
        Ok(read_dir) => read_dir,
        Err(_) => return entries,
    };

    let mut archive_dirs = read_dir
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            path.is_dir().then_some(path)
        })
        .collect::<Vec<_>>();
    archive_dirs.sort_by(|left, right| right.cmp(left));

    for archive_dir in archive_dirs {
        let run_path = archive_dir.join("run.json");
        let Some(run) = load_persisted_artifacts_run(run_path.clone()) else {
            continue;
        };
        let archive_micros = persisted_history_archive_micros(&archive_dir);
        let updated = persisted_run_updated_label(&run);
        let counts = make_detail(&run);
        entries.push(SavedArtifactsRunEntry {
            kind: SavedArtifactsRunKind::Archived,
            label: format_saved_run_relative_label_from_micros(archive_micros, now_micros),
            detail: saved_run_detail_label(archive_micros, &updated, &counts),
            run_path: Some(run_path.to_string_lossy().to_string()),
            archive_micros,
        });
    }

    entries
}

/// When no archived snapshots exist, surface the current `run.json` (written
/// continuously by the flush loop) so the history popup is never empty while
/// the user has active data.
fn current_run_as_history_entry<F>(
    run_path: &Path,
    make_detail: F,
) -> Option<SavedArtifactsRunEntry>
where
    F: FnOnce(&PersistedArtifactsRun) -> String,
{
    let run = load_persisted_artifacts_run(run_path.to_path_buf())?;
    let updated = persisted_run_updated_label(&run);
    let counts = make_detail(&run);
    let modified_micros = fs::metadata(run_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_micros());
    let now_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    Some(SavedArtifactsRunEntry {
        kind: SavedArtifactsRunKind::Archived,
        label: format_saved_run_relative_label_from_micros(modified_micros, now_micros),
        detail: saved_run_detail_label(modified_micros, &updated, &counts),
        run_path: Some(run_path.to_string_lossy().to_string()),
        archive_micros: modified_micros,
    })
}

fn mission_run_counts(run: &PersistedArtifactsRun, mission_id: &str) -> (usize, usize, usize) {
    let messages = run
        .messages
        .iter()
        .filter(|message| message.mission_id.as_deref() == Some(mission_id))
        .count();
    let patches = run
        .patches
        .iter()
        .filter(|patch| patch.mission_id.as_deref() == Some(mission_id))
        .count();
    let evidence = run
        .evidence
        .iter()
        .filter(|item| item.mission_id.as_deref() == Some(mission_id))
        .count();
    (messages, patches, evidence)
}

fn ad_hoc_run_counts(run: &PersistedArtifactsRun, agent_id: &str) -> (usize, usize, usize) {
    let messages = run
        .messages
        .iter()
        .filter(|message| {
            message.mission_id.is_none()
                && (message.agent_id.is_none() || message.agent_id.as_deref() == Some(agent_id))
        })
        .count();
    let patches = run
        .patches
        .iter()
        .filter(|patch| patch.mission_id.is_none() && patch.agent_id == agent_id)
        .count();
    let evidence = run
        .evidence
        .iter()
        .filter(|item| item.mission_id.is_none() && item.agent_id.as_deref() == Some(agent_id))
        .count();
    (messages, patches, evidence)
}

pub fn artifacts_history_entries(state: &AppState) -> Vec<SavedArtifactsRunEntry> {
    let mut entries = vec![SavedArtifactsRunEntry {
        kind: SavedArtifactsRunKind::Current,
        label: "current / latest saved run".into(),
        detail: "Use the live thread when available, otherwise the latest saved run.".into(),
        run_path: None,
        archive_micros: None,
    }];

    if let Some(mission_id) = state.agents.selected_context_mission() {
        let archived =
            history_entries_for_root(persisted_mission_history_root(state, mission_id), |run| {
                let (messages, patches, evidence) = mission_run_counts(run, mission_id);
                format!("{messages} msgs · {patches} patches · {evidence} evidence")
            });
        if archived.is_empty() {
            // No archived snapshots yet — surface the current run.json so the
            // user sees their data before they've ever reset context.
            let run_path = persisted_mission_run_path(state, mission_id);
            if let Some(entry) = current_run_as_history_entry(&run_path, |run| {
                let (messages, patches, evidence) = mission_run_counts(run, mission_id);
                format!("{messages} msgs · {patches} patches · {evidence} evidence")
            }) {
                entries.push(entry);
            }
        } else {
            entries.extend(archived);
        }
    } else if let Some(agent_id) = state.agents.selected_context_agent() {
        let archived =
            history_entries_for_root(persisted_ad_hoc_history_root(state, agent_id), |run| {
                let (messages, patches, evidence) = ad_hoc_run_counts(run, agent_id);
                format!("{messages} msgs · {patches} patches · {evidence} evidence")
            });
        if archived.is_empty() {
            let run_path = persisted_ad_hoc_run_path(state, agent_id);
            if let Some(entry) = current_run_as_history_entry(&run_path, |run| {
                let (messages, patches, evidence) = ad_hoc_run_counts(run, agent_id);
                format!("{messages} msgs · {patches} patches · {evidence} evidence")
            }) {
                entries.push(entry);
            }
        } else {
            entries.extend(archived);
        }
    }

    entries
}

pub fn artifacts_history_visible_entries(state: &AppState) -> Vec<SavedArtifactsRunEntry> {
    let entries = artifacts_history_entries(state);
    let filter = state.agents.artifacts_history_filter;
    let Some(_) = saved_run_history_filter_window_micros(filter) else {
        return entries;
    };
    let selected_path = state.agents.artifacts_selected_saved_run_path.as_deref();
    let now_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    entries
        .into_iter()
        .filter(|entry| {
            entry.run_path.as_deref() == selected_path
                || saved_run_entry_matches_filter(entry, filter, now_micros)
        })
        .collect()
}

pub fn artifacts_history_prunable_entries(state: &AppState) -> Vec<SavedArtifactsRunEntry> {
    let filter = state.agents.artifacts_history_filter;
    let now_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    artifacts_history_entries(state)
        .into_iter()
        .filter(|entry| matches!(entry.kind, SavedArtifactsRunKind::Archived))
        .filter(|entry| saved_run_entry_matches_filter(entry, filter, now_micros))
        .collect()
}

fn artifacts_history_entry_index_for_path(
    entries: &[SavedArtifactsRunEntry],
    selected_path: Option<&str>,
) -> usize {
    entries
        .iter()
        .position(|entry| entry.run_path.as_deref() == selected_path)
        .unwrap_or(0)
        .min(entries.len().saturating_sub(1))
}

fn selected_archived_run_for_current_context(
    state: &AppState,
) -> Option<(SavedArtifactsRunEntry, PersistedArtifactsRun)> {
    let selected_path = state.agents.artifacts_selected_saved_run_path.as_deref()?;
    let entry = artifacts_history_entries(state)
        .into_iter()
        .find(|entry| entry.run_path.as_deref() == Some(selected_path))?;
    let run = load_persisted_artifacts_run(PathBuf::from(selected_path))?;
    Some((entry, run))
}

pub fn artifacts_selected_history_entry(state: &AppState) -> usize {
    let entries = artifacts_history_entries(state);
    artifacts_history_entry_index_for_path(
        &entries,
        state.agents.artifacts_selected_saved_run_path.as_deref(),
    )
}

pub fn artifacts_selected_visible_history_entry(state: &AppState) -> usize {
    let entries = artifacts_history_visible_entries(state);
    artifacts_history_entry_index_for_path(
        &entries,
        state.agents.artifacts_selected_saved_run_path.as_deref(),
    )
}

pub fn artifacts_history_summary_label(state: &AppState) -> String {
    if let Some((entry, _)) = selected_archived_run_for_current_context(state) {
        entry.label
    } else {
        "current / latest saved run".into()
    }
}

// ---------------------------------------------------------------------------
// Global Artifact Archive
// ---------------------------------------------------------------------------

const GLOBAL_ARCHIVE_PREVIEW_CHARS: usize = 120;
/// Full-text chunk size for BM25 indexing (RAG best practice: 200-1000 tokens).
const GLOBAL_ARCHIVE_FULLTEXT_CHARS: usize = 2000;

struct ArchiveSourceCtx<'a> {
    source: &'a str,
    source_id: &'a str,
    source_kind: GlobalArchiveSourceKind,
}

/// Tokenize text into lowercased words for BM25 scoring.
fn tokenize_for_bm25(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-')
        .filter(|w| w.len() >= 2)
        .map(|w| w.to_ascii_lowercase())
        .collect()
}

/// Build the search haystack from full content (for fuzzy) and extract tokens (for BM25).
fn build_search_fields(
    kind: &str,
    owner: &str,
    source: &str,
    full_text: &str,
) -> (String, Vec<String>) {
    let hay = format!(
        "{kind} {owner} {source} {}",
        summarize_text_preview(full_text, GLOBAL_ARCHIVE_FULLTEXT_CHARS)
    )
    .to_ascii_lowercase();
    let tokens = tokenize_for_bm25(&hay);
    (hay, tokens)
}

fn extract_run_entries(
    run: &PersistedArtifactsRun,
    run_path: &Path,
    ctx: &ArchiveSourceCtx<'_>,
    archive_micros: Option<u128>,
    now_micros: u128,
    out: &mut Vec<GlobalArchiveEntry>,
) {
    let run_path_str = run_path.to_string_lossy().to_string();
    let time_label = format_saved_run_relative_label_from_micros(archive_micros, now_micros);

    // For swarm missions without explicit kind tags, infer special messages:
    // - first reply from the planner (assigned_agents[0]) is the plan
    // - last reply from the planner is the synthesis report
    let (plan_msg_idx, synth_msg_idx) = if run.swarm {
        let planner = run.assigned_agents.first().map(String::as_str);
        let plan = planner.and_then(|pid| {
            run.messages
                .iter()
                .enumerate()
                .find(|(_, m)| m.agent_id.as_deref() == Some(pid))
                .map(|(i, _)| i)
        });
        let synth = planner.and_then(|pid| {
            run.messages
                .iter()
                .enumerate()
                .rev()
                .find(|(_, m)| m.agent_id.as_deref() == Some(pid))
                .map(|(i, _)| i)
        });
        // If plan and synth point to the same message, it's just a synth (single planner reply).
        if plan == synth {
            (None, synth)
        } else {
            (plan, synth)
        }
    } else {
        (None, None)
    };

    for (idx, msg) in run.messages.iter().enumerate() {
        if msg.agent_id.as_deref() == Some("swarm") {
            continue;
        }
        let kind: &'static str = if msg.agent_id.is_none() {
            "PROMPT"
        } else if msg.kind.as_deref() == Some("synth") || synth_msg_idx == Some(idx) {
            "SYNTH"
        } else if msg.kind.as_deref() == Some("plan") || plan_msg_idx == Some(idx) {
            "PLAN"
        } else {
            "REPLY"
        };
        let owner = msg
            .agent_id
            .as_deref()
            .map(crate::swarm::compact_agent_display_id)
            .unwrap_or_else(|| "You".into());
        let preview = summarize_text_preview(&msg.text, GLOBAL_ARCHIVE_PREVIEW_CHARS);
        let (search_hay, search_tokens) = build_search_fields(kind, &owner, ctx.source, &msg.text);
        out.push(GlobalArchiveEntry {
            kind,
            owner,
            preview,
            source: ctx.source.to_string(),
            source_id: ctx.source_id.to_string(),
            source_kind: ctx.source_kind.clone(),
            time_label: time_label.clone(),
            archive_micros,
            run_path: run_path_str.clone(),
            artifact_index: idx,
            search_hay,
            search_tokens,
        });
    }

    for (idx, patch) in run.patches.iter().enumerate() {
        let owner = if patch.agent_id.is_empty() {
            "system".to_string()
        } else {
            crate::swarm::compact_agent_display_id(&patch.agent_id)
        };
        let preview = if !patch.title.is_empty() {
            summarize_text_preview(&patch.title, GLOBAL_ARCHIVE_PREVIEW_CHARS)
        } else {
            summarize_text_preview(&patch.summary, GLOBAL_ARCHIVE_PREVIEW_CHARS)
        };
        let full_text = if !patch.title.is_empty() {
            &patch.title
        } else {
            &patch.summary
        };
        let (search_hay, search_tokens) =
            build_search_fields("PATCH", &owner, ctx.source, full_text);
        out.push(GlobalArchiveEntry {
            kind: "PATCH",
            owner,
            preview,
            source: ctx.source.to_string(),
            source_id: ctx.source_id.to_string(),
            source_kind: ctx.source_kind.clone(),
            time_label: time_label.clone(),
            archive_micros,
            run_path: run_path_str.clone(),
            artifact_index: idx,
            search_hay,
            search_tokens,
        });
    }

    for (idx, item) in run.evidence.iter().enumerate() {
        let owner = item
            .agent_id
            .as_deref()
            .map(crate::swarm::compact_agent_display_id)
            .unwrap_or_else(|| "system".into());
        let preview = summarize_text_preview(&item.title, GLOBAL_ARCHIVE_PREVIEW_CHARS);
        let (search_hay, search_tokens) =
            build_search_fields("EVIDENCE", &owner, ctx.source, &item.title);
        out.push(GlobalArchiveEntry {
            kind: "EVIDENCE",
            owner,
            preview,
            source: ctx.source.to_string(),
            source_id: ctx.source_id.to_string(),
            source_kind: ctx.source_kind.clone(),
            time_label: time_label.clone(),
            archive_micros,
            run_path: run_path_str.clone(),
            artifact_index: idx,
            search_hay,
            search_tokens,
        });
    }
}

/// Return file mtime as microseconds since epoch, or `None` if unavailable.
fn file_mtime_micros(path: &Path) -> Option<u128> {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_micros())
}

/// Collect all run.json paths with their mtimes from the agents directory.
fn discover_run_files(agents_root: &Path) -> Vec<(PathBuf, Option<u128>)> {
    let mut files = Vec::new();

    // Mission runs.
    let runs_root = agents_root.join("runs");
    if let Ok(read_dir) = fs::read_dir(&runs_root) {
        for entry in read_dir.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // Current run.json.
            let run_path = path.join("run.json");
            if run_path.exists() {
                let mtime = file_mtime_micros(&run_path);
                files.push((run_path, mtime));
            }
            // History subdirectories.
            let history = path.join("history");
            if let Ok(hist_dir) = fs::read_dir(&history) {
                for hentry in hist_dir.filter_map(|e| e.ok()) {
                    let hp = hentry.path();
                    if hp.is_dir() {
                        let rp = hp.join("run.json");
                        if rp.exists() {
                            let mtime = file_mtime_micros(&rp);
                            files.push((rp, mtime));
                        }
                    }
                }
            }
        }
    }

    // Ad-hoc runs.
    let adhoc_root = agents_root.join("ad-hoc");
    if let Ok(read_dir) = fs::read_dir(&adhoc_root) {
        for entry in read_dir.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let run_path = path.join("run.json");
            if run_path.exists() {
                let mtime = file_mtime_micros(&run_path);
                files.push((run_path, mtime));
            }
            let history = path.join("history");
            if let Ok(hist_dir) = fs::read_dir(&history) {
                for hentry in hist_dir.filter_map(|e| e.ok()) {
                    let hp = hentry.path();
                    if hp.is_dir() {
                        let rp = hp.join("run.json");
                        if rp.exists() {
                            let mtime = file_mtime_micros(&rp);
                            files.push((rp, mtime));
                        }
                    }
                }
            }
        }
    }

    files
}

/// Build the global archive index with incremental update support.
///
/// If `prev_index` is non-empty, entries from unchanged run.json files (same
/// path and mtime) are carried forward without re-parsing.  Only new or
/// modified files trigger JSON deserialization.
pub fn build_global_archive_index(state: &AppState) -> Vec<GlobalArchiveEntry> {
    let now_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    let agents_root = state.workspace_root.join(".nit").join("agents");

    // Discover all run.json files and their mtimes.
    let run_files = discover_run_files(&agents_root);

    // Build a lookup of cached entries keyed by (run_path, mtime) from the
    // previous index so unchanged files can be carried forward.
    let prev = &state.agents.global_archive_index;
    let mut cache: std::collections::HashMap<(&str, Option<u128>), Vec<usize>> =
        std::collections::HashMap::new();
    for (idx, entry) in prev.iter().enumerate() {
        cache
            .entry((entry.run_path.as_str(), entry.archive_micros))
            .or_default()
            .push(idx);
    }

    let mut entries = Vec::new();

    for (run_path, mtime) in &run_files {
        let run_path_str = run_path.to_string_lossy().to_string();

        // Incremental: if this file was in the previous index with the same
        // mtime, carry forward cached entries (skip JSON parsing).
        if let Some(cached_indices) = cache.get(&(run_path_str.as_str(), *mtime)) {
            if !cached_indices.is_empty() {
                for &idx in cached_indices {
                    if let Some(entry) = prev.get(idx) {
                        // Update the time label to reflect current time, clone the rest.
                        let mut refreshed = entry.clone();
                        refreshed.time_label = format_saved_run_relative_label_from_micros(
                            entry.archive_micros,
                            now_micros,
                        );
                        entries.push(refreshed);
                    }
                }
                continue;
            }
        }

        // New or modified file: parse and extract.
        let Some(run) = load_persisted_artifacts_run(run_path.clone()) else {
            continue;
        };

        // Determine source context from the path.
        let (source, source_id, source_kind) = resolve_run_source(state, run_path, &agents_root);
        let archive_micros = run_path
            .parent()
            .and_then(persisted_history_archive_micros)
            .or(*mtime);
        let ctx = ArchiveSourceCtx {
            source: &source,
            source_id: &source_id,
            source_kind,
        };
        extract_run_entries(
            &run,
            run_path,
            &ctx,
            archive_micros,
            now_micros,
            &mut entries,
        );
    }

    entries.sort_by(|a, b| b.archive_micros.cmp(&a.archive_micros));
    entries
}

/// Resolve source metadata from a run.json path.
fn resolve_run_source(
    state: &AppState,
    run_path: &Path,
    agents_root: &Path,
) -> (String, String, GlobalArchiveSourceKind) {
    let rel = run_path.strip_prefix(agents_root).unwrap_or(run_path);
    let components: Vec<&str> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    // Pattern: runs/{mission_id}/[history/{ts}/]run.json
    if components.first() == Some(&"runs") {
        if let Some(&mission_id) = components.get(1) {
            let source = state
                .agents
                .missions
                .iter()
                .find(|m| m.id == mission_id)
                .map(|m| {
                    if m.title.is_empty() {
                        format!("mission: {mission_id}")
                    } else {
                        format!("mission: {}", m.title)
                    }
                })
                .unwrap_or_else(|| format!("mission: {mission_id}"));
            return (
                source,
                mission_id.to_string(),
                GlobalArchiveSourceKind::Mission,
            );
        }
    }
    // Pattern: ad-hoc/{agent_id}/[history/{ts}/]run.json
    if components.first() == Some(&"ad-hoc") {
        if let Some(&agent_id) = components.get(1) {
            let source = format!("ad-hoc: {agent_id}");
            return (source, agent_id.to_string(), GlobalArchiveSourceKind::AdHoc);
        }
    }
    // Fallback.
    let name = run_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    (
        name.to_string(),
        name.to_string(),
        GlobalArchiveSourceKind::AdHoc,
    )
}

/// Compute BM25 score for a single document against query terms.
///
/// BM25 parameters: k1 controls term frequency saturation, b controls
/// document length normalization. Standard defaults: k1=1.2, b=0.75.
fn bm25_score(
    doc_tokens: &[String],
    query_terms: &[String],
    _doc_count: usize,
    avg_doc_len: f64,
    idf_map: &std::collections::HashMap<String, f64>,
) -> f64 {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;
    let dl = doc_tokens.len() as f64;
    let mut score = 0.0;

    for term in query_terms {
        let idf = idf_map.get(term.as_str()).copied().unwrap_or(0.0);
        if idf <= 0.0 {
            continue;
        }
        // Term frequency in this document.
        let tf = doc_tokens
            .iter()
            .filter(|t| t.as_str() == term.as_str())
            .count() as f64;
        if tf == 0.0 {
            continue;
        }
        // BM25 TF component with length normalization.
        let numerator = tf * (K1 + 1.0);
        let denominator = tf + K1 * (1.0 - B + B * dl / avg_doc_len.max(1.0));
        score += idf * numerator / denominator;
    }

    // BM25+ delta: ensure matching terms always contribute a minimum.
    if score > 0.0 {
        let delta = 0.5
            * query_terms
                .iter()
                .filter(|t| doc_tokens.contains(t))
                .count() as f64;
        score += delta;
    }

    score
}

/// Build IDF (Inverse Document Frequency) map for query terms across the corpus.
fn build_idf_map(
    index: &[GlobalArchiveEntry],
    query_terms: &[String],
    time_filtered: &[usize],
) -> std::collections::HashMap<String, f64> {
    let n = time_filtered.len().max(1) as f64;
    let mut idf_map = std::collections::HashMap::new();
    for term in query_terms {
        let df = time_filtered
            .iter()
            .filter(|&&idx| {
                index
                    .get(idx)
                    .is_some_and(|e| e.search_tokens.contains(term))
            })
            .count() as f64;
        // IDF formula: ln((N - df + 0.5) / (df + 0.5) + 1)
        let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
        idf_map.insert(term.clone(), idf);
    }
    idf_map
}

pub fn filter_global_archive(
    index: &[GlobalArchiveEntry],
    query: &str,
    filter: SavedRunHistoryFilter,
) -> Vec<(i64, usize)> {
    let now_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    let window = saved_run_history_filter_window_micros(filter);

    // Step 1: Time-window filter.
    let time_filtered: Vec<usize> = index
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            if let Some(window_micros) = window {
                if let Some(archive) = entry.archive_micros {
                    now_micros.saturating_sub(archive) <= window_micros
                } else {
                    true
                }
            } else {
                true
            }
        })
        .map(|(idx, _)| idx)
        .collect();

    // No query: return all time-filtered entries sorted by recency (index order).
    let needle = query.to_ascii_lowercase();
    if needle.is_empty() {
        return time_filtered.into_iter().map(|idx| (0i64, idx)).collect();
    }

    // Step 2: Tokenize query and build BM25 corpus statistics.
    let query_terms = tokenize_for_bm25(&needle);
    let needle_bytes = needle.as_bytes();

    let avg_doc_len = if time_filtered.is_empty() {
        1.0
    } else {
        time_filtered
            .iter()
            .filter_map(|&idx| index.get(idx))
            .map(|e| e.search_tokens.len() as f64)
            .sum::<f64>()
            / time_filtered.len() as f64
    };
    let idf_map = build_idf_map(index, &query_terms, &time_filtered);
    let doc_count = time_filtered.len();

    // Step 3: Hybrid scoring — BM25 (relevance) + fuzzy (typo tolerance) + recency.
    let mut results: Vec<(i64, usize)> = time_filtered
        .iter()
        .filter_map(|&idx| {
            let entry = index.get(idx)?;

            // BM25 score (token-level relevance).
            let bm25 = bm25_score(
                &entry.search_tokens,
                &query_terms,
                doc_count,
                avg_doc_len,
                &idf_map,
            );

            // Fuzzy score (character-level typo tolerance).
            let fuzzy = crate::fuzzy_search_runner::fuzzy_score_bytes(
                entry.search_hay.as_bytes(),
                needle_bytes,
            )
            .map(|(s, _)| s)
            .unwrap_or(0);

            // Must match on at least one signal.
            if bm25 <= 0.0 && fuzzy <= 0 {
                return None;
            }

            // Recency boost: newer artifacts get a small bonus.
            let recency_boost = entry
                .archive_micros
                .map(|micros| {
                    let age_hours = now_micros.saturating_sub(micros) / (3_600 * 1_000_000);
                    // Decay: 10 points for recent, tapering to 0 over ~30 days.
                    10.0 / (1.0 + age_hours as f64 / 168.0)
                })
                .unwrap_or(0.0);

            // Combined score: BM25 * 100 (dominant) + fuzzy + recency.
            let combined = (bm25 * 100.0) as i64 + fuzzy + recency_boost as i64;
            Some((combined, idx))
        })
        .collect();

    results.sort_by(|a, b| b.0.cmp(&a.0));
    results
}

fn persisted_patch_diff_excerpt(patch: &PersistedPatchRecord, path: Option<&str>) -> String {
    if !patch.diff.trim().is_empty() {
        return patch.diff.clone();
    }
    let Some(path) = path else {
        return String::new();
    };
    fs::read_to_string(path).unwrap_or_default()
}

pub fn artifact_list_widths(width: usize) -> Vec<usize> {
    // Two leading glyphs + spacer: `>➜ `
    let cols_total = width.saturating_sub(3);
    // Prefer giving preview text as much space as possible.
    allocate_columns(cols_total, &[6, 6, 8, 12], &[8, 8, 10, 50], 3)
}

/// Root prompt glyph.
const PROMPT_GLYPH: char = '→';

fn artifact_card_row(
    card: &ArtifactCard,
    widths: &[usize],
    selected: bool,
    is_child: bool,
) -> String {
    let selected_marker = if selected { '>' } else { ' ' };
    if is_child {
        // Child row: indented with tree connector ↳.
        let glyph = if selected { cursor_glyph() } else { '↳' };
        let child_preview_width = widths.get(3).copied().unwrap_or(0);
        format!(
            "{selected_marker}  {glyph} {} {} {} {}",
            fit_left(card.kind, *widths.first().unwrap_or(&0)),
            fit_left(card.at.as_str(), *widths.get(1).unwrap_or(&0)),
            fit_left(card.owner.as_str(), *widths.get(2).unwrap_or(&0)),
            fit_left(card.preview.as_str(), child_preview_width.saturating_sub(2)),
        )
    } else {
        // Root row (prompt): distinct glyph →.
        let glyph = if selected {
            cursor_glyph()
        } else {
            PROMPT_GLYPH
        };
        format!(
            "{selected_marker}{glyph} {} {} {} {}",
            fit_left(card.kind, *widths.first().unwrap_or(&0)),
            fit_left(card.at.as_str(), *widths.get(1).unwrap_or(&0)),
            fit_left(card.owner.as_str(), *widths.get(2).unwrap_or(&0)),
            fit_left(card.preview.as_str(), *widths.get(3).unwrap_or(&0)),
        )
    }
}

/// Group reply indices under their correct prompt index.
///
/// For replies that have a `prompt_msg_idx` set (explicit parent tracking), the reply is
/// placed under that prompt. For replies without it, falls back to the nearest preceding
/// prompt by index (binary search).
///
/// Returns a list of `(Option<prompt_idx>, Vec<reply_indices>)` groups.
fn group_replies_under_prompts(
    prompt_indices: &[usize],
    reply_indices: &[usize],
) -> Vec<(Option<usize>, Vec<usize>)> {
    group_replies_under_prompts_with_hints(prompt_indices, reply_indices, &[])
}

/// Like `group_replies_under_prompts` but accepts explicit parent hints.
/// `parent_hints` is parallel to `reply_indices`; each entry is `Some(prompt_msg_idx)` if the
/// reply explicitly knows its parent prompt, or `None` to fall back to positional grouping.
fn group_replies_under_prompts_with_hints(
    prompt_indices: &[usize],
    reply_indices: &[usize],
    parent_hints: &[Option<usize>],
) -> Vec<(Option<usize>, Vec<usize>)> {
    if prompt_indices.is_empty() {
        if reply_indices.is_empty() {
            return Vec::new();
        }
        return vec![(None, reply_indices.to_vec())];
    }

    // Build one group per prompt (in order).
    let mut groups: Vec<(Option<usize>, Vec<usize>)> = prompt_indices
        .iter()
        .map(|&idx| (Some(idx), Vec::new()))
        .collect();

    // Build a quick lookup: prompt_msg_idx → position in groups vec.
    let prompt_pos: std::collections::HashMap<usize, usize> = prompt_indices
        .iter()
        .enumerate()
        .map(|(pos, &idx)| (idx, pos))
        .collect();

    for (i, &reply_idx) in reply_indices.iter().enumerate() {
        // 1) Try explicit parent hint first.
        let hint = parent_hints.get(i).copied().flatten();
        if let Some(parent_idx) = hint {
            if let Some(&slot) = prompt_pos.get(&parent_idx) {
                let offset = if groups.first().is_some_and(|g| g.0.is_none()) {
                    1
                } else {
                    0
                };
                groups[slot + offset].1.push(reply_idx);
                continue;
            }
        }

        // 2) Fallback: binary search for the rightmost prompt_idx <= reply_idx.
        let slot = match prompt_indices.binary_search(&reply_idx) {
            Ok(pos) => pos,
            Err(pos) => {
                if pos == 0 {
                    usize::MAX // sentinel for "orphan"
                } else {
                    pos - 1
                }
            }
        };
        if slot == usize::MAX {
            if groups.first().is_some_and(|g| g.0.is_none()) {
                groups[0].1.push(reply_idx);
            } else {
                groups.insert(0, (None, vec![reply_idx]));
            }
        } else {
            let offset = if groups.first().is_some_and(|g| g.0.is_none()) {
                1
            } else {
                0
            };
            groups[slot + offset].1.push(reply_idx);
        }
    }

    groups
}

fn build_mission_cards(
    state: &AppState,
    mission_id: &str,
    preview_chars: usize,
) -> Vec<ArtifactCard> {
    // Two-pass grouping: collect prompts first, then assign each reply to the prompt
    // whose message index is the highest that is still <= the reply's index.
    // This correctly handles out-of-order replies (e.g. reply to prompt 1 arriving
    // after prompt 2 was already answered).
    let mut prompt_indices: Vec<usize> = Vec::new();
    let mut reply_indices: Vec<usize> = Vec::new();
    let mut parent_hints: Vec<Option<usize>> = Vec::new();
    for (idx, message) in state.agents.messages.iter().enumerate() {
        if message.mission_id.as_deref() != Some(mission_id) {
            continue;
        }
        // Skip swarm meta messages — they're internal bookkeeping,
        // not real agent replies.
        if message.agent_id.as_deref() == Some("swarm") {
            continue;
        }
        if matches!(message.channel, nit_core::AgentChannel::Broadcast)
            && message.agent_id.is_some()
        {
            continue;
        }
        if message.agent_id.is_none() {
            prompt_indices.push(idx);
        } else {
            reply_indices.push(idx);
            parent_hints.push(message.prompt_msg_idx);
        }
    }
    let groups =
        group_replies_under_prompts_with_hints(&prompt_indices, &reply_indices, &parent_hints);

    let mut patch_indices = state
        .agents
        .patches
        .iter()
        .enumerate()
        .filter_map(|(idx, patch)| (patch.mission_id.as_deref() == Some(mission_id)).then_some(idx))
        .collect::<Vec<_>>();
    let mut evidence_indices = state
        .agents
        .evidence
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| (item.mission_id.as_deref() == Some(mission_id)).then_some(idx))
        .collect::<Vec<_>>();

    if patch_indices.len() > ARTIFACT_CARD_LIMIT_PATCHES {
        patch_indices = patch_indices.split_off(patch_indices.len() - ARTIFACT_CARD_LIMIT_PATCHES);
    }
    if evidence_indices.len() > ARTIFACT_CARD_LIMIT_EVIDENCE {
        evidence_indices =
            evidence_indices.split_off(evidence_indices.len() - ARTIFACT_CARD_LIMIT_EVIDENCE);
    }

    let mut cards = Vec::new();
    for (prompt_idx, mut reply_indices) in groups {
        if let Some(idx) = prompt_idx {
            if let Some(message) = state.agents.messages.get(idx) {
                cards.push(ArtifactCard {
                    kind: "PROMPT",
                    at: message.at.clone(),
                    owner: "You".into(),
                    preview: summarize_text_preview(message.text.as_str(), preview_chars),
                    reference: ArtifactRef::Message { idx },
                });
            }
        }
        if reply_indices.len() > ARTIFACT_CARD_LIMIT_REPLIES {
            reply_indices =
                reply_indices.split_off(reply_indices.len() - ARTIFACT_CARD_LIMIT_REPLIES);
        }
        for idx in reply_indices {
            if let Some(message) = state.agents.messages.get(idx) {
                let owner = message.agent_id.as_deref().unwrap_or("agent");
                let kind = if message.kind.as_deref() == Some("synth") {
                    "SYNTH"
                } else {
                    "REPLY"
                };
                cards.push(ArtifactCard {
                    kind,
                    at: message.at.clone(),
                    owner: owner.into(),
                    preview: summarize_text_preview(message.text.as_str(), preview_chars),
                    reference: ArtifactRef::Message { idx },
                });
            }
        }
    }
    // Patches and evidence are appended after the message tree.
    for idx in patch_indices.into_iter().rev() {
        if let Some(patch) = state.agents.patches.get(idx) {
            cards.push(ArtifactCard {
                kind: "PATCH",
                at: patch.status.label().into(),
                owner: patch.agent_id.clone(),
                preview: patch.title.clone(),
                reference: ArtifactRef::Patch { idx },
            });
        }
    }
    for idx in evidence_indices.into_iter().rev() {
        if let Some(item) = state.agents.evidence.get(idx) {
            let owner = item.agent_id.as_deref().unwrap_or("system");
            cards.push(ArtifactCard {
                kind: "EVIDENCE",
                at: String::new(),
                owner: owner.into(),
                preview: item.title.clone(),
                reference: ArtifactRef::Evidence { idx },
            });
        }
    }
    cards
}

fn build_ad_hoc_cards(state: &AppState, agent_id: &str, preview_chars: usize) -> Vec<ArtifactCard> {
    let is_own_or_clone = |id: Option<&str>| -> bool {
        id == Some(agent_id) || id.is_some_and(|id| chat_clone_base_id(id) == Some(agent_id))
    };
    // Two-pass grouping: assign each reply to the prompt whose message index
    // is the highest that is still <= the reply's index.
    let mut prompt_indices: Vec<usize> = Vec::new();
    let mut reply_indices_vec: Vec<usize> = Vec::new();
    let mut parent_hints: Vec<Option<usize>> = Vec::new();
    for (idx, message) in state.agents.messages.iter().enumerate() {
        if message.mission_id.is_some() {
            continue;
        }
        if message.agent_id.is_none() {
            prompt_indices.push(idx);
        } else if is_own_or_clone(message.agent_id.as_deref()) {
            reply_indices_vec.push(idx);
            parent_hints.push(message.prompt_msg_idx);
        }
    }
    let groups =
        group_replies_under_prompts_with_hints(&prompt_indices, &reply_indices_vec, &parent_hints);

    let mut patch_indices = state
        .agents
        .patches
        .iter()
        .enumerate()
        .filter_map(|(idx, patch)| {
            (patch.mission_id.is_none()
                && (patch.agent_id == agent_id
                    || chat_clone_base_id(&patch.agent_id) == Some(agent_id)))
            .then_some(idx)
        })
        .collect::<Vec<_>>();

    if patch_indices.len() > ARTIFACT_CARD_LIMIT_PATCHES {
        patch_indices = patch_indices.split_off(patch_indices.len() - ARTIFACT_CARD_LIMIT_PATCHES);
    }

    let mut cards = Vec::new();
    for (prompt_idx, mut reply_indices) in groups {
        if let Some(idx) = prompt_idx {
            if let Some(message) = state.agents.messages.get(idx) {
                cards.push(ArtifactCard {
                    kind: "PROMPT",
                    at: message.at.clone(),
                    owner: "You".into(),
                    preview: summarize_text_preview(message.text.as_str(), preview_chars),
                    reference: ArtifactRef::Message { idx },
                });
            }
        }
        if reply_indices.len() > ARTIFACT_CARD_LIMIT_REPLIES {
            reply_indices =
                reply_indices.split_off(reply_indices.len() - ARTIFACT_CARD_LIMIT_REPLIES);
        }
        for idx in reply_indices {
            if let Some(message) = state.agents.messages.get(idx) {
                let kind = if message.kind.as_deref() == Some("synth") {
                    "SYNTH"
                } else {
                    "REPLY"
                };
                cards.push(ArtifactCard {
                    kind,
                    at: message.at.clone(),
                    owner: agent_id.into(),
                    preview: summarize_text_preview(message.text.as_str(), preview_chars),
                    reference: ArtifactRef::Message { idx },
                });
            }
        }
    }
    for idx in patch_indices.into_iter().rev() {
        if let Some(patch) = state.agents.patches.get(idx) {
            cards.push(ArtifactCard {
                kind: "PATCH",
                at: patch.status.label().into(),
                owner: patch.agent_id.clone(),
                preview: patch.title.clone(),
                reference: ArtifactRef::Patch { idx },
            });
        }
    }
    cards
}

fn build_persisted_mission_cards(
    run: &PersistedArtifactsRun,
    mission_id: &str,
    preview_chars: usize,
) -> Vec<ArtifactCard> {
    // Two-pass grouping: assign each reply to its nearest preceding prompt.
    let filtered: Vec<&AgentMessage> = run
        .messages
        .iter()
        .filter(|m| m.mission_id.as_deref() == Some(mission_id))
        .collect();
    let prompt_positions: Vec<usize> = filtered
        .iter()
        .enumerate()
        .filter_map(|(i, m)| m.agent_id.is_none().then_some(i))
        .collect();
    let reply_positions: Vec<usize> = filtered
        .iter()
        .enumerate()
        .filter_map(|(i, m)| m.agent_id.is_some().then_some(i))
        .collect();
    let groups = group_replies_under_prompts(&prompt_positions, &reply_positions);

    let mut patches = run
        .patches
        .iter()
        .filter(|patch| patch.mission_id.as_deref() == Some(mission_id))
        .cloned()
        .collect::<Vec<_>>();
    let mut evidence = run
        .evidence
        .iter()
        .filter(|item| item.mission_id.as_deref() == Some(mission_id))
        .cloned()
        .collect::<Vec<_>>();

    if patches.len() > ARTIFACT_CARD_LIMIT_PATCHES {
        patches = patches.split_off(patches.len() - ARTIFACT_CARD_LIMIT_PATCHES);
    }
    if evidence.len() > ARTIFACT_CARD_LIMIT_EVIDENCE {
        evidence = evidence.split_off(evidence.len() - ARTIFACT_CARD_LIMIT_EVIDENCE);
    }

    // Infer plan/synthesis: first and last reply from the planner in a swarm mission.
    let (plan_pos, synth_pos) = if run.swarm {
        let planner = run.assigned_agents.first().map(String::as_str);
        let plan = planner.and_then(|pid| {
            filtered
                .iter()
                .enumerate()
                .find(|(_, m)| m.agent_id.as_deref() == Some(pid))
                .map(|(i, _)| i)
        });
        let synth = planner.and_then(|pid| {
            filtered
                .iter()
                .enumerate()
                .rev()
                .find(|(_, m)| m.agent_id.as_deref() == Some(pid))
                .map(|(i, _)| i)
        });
        if plan == synth {
            (None, synth)
        } else {
            (plan, synth)
        }
    } else {
        (None, None)
    };

    let mut cards = Vec::new();
    for (prompt_pos, mut child_positions) in groups {
        if let Some(pos) = prompt_pos {
            if let Some(message) = filtered.get(pos) {
                cards.push(ArtifactCard {
                    kind: "PROMPT",
                    at: message.at.clone(),
                    owner: "You".into(),
                    preview: summarize_text_preview(message.text.as_str(), preview_chars),
                    reference: ArtifactRef::PersistedMessage {
                        message: (*message).clone(),
                    },
                });
            }
        }
        if child_positions.len() > ARTIFACT_CARD_LIMIT_REPLIES {
            child_positions =
                child_positions.split_off(child_positions.len() - ARTIFACT_CARD_LIMIT_REPLIES);
        }
        for pos in child_positions {
            if let Some(message) = filtered.get(pos) {
                let owner = message.agent_id.as_deref().unwrap_or("agent").to_string();
                let kind = if message.kind.as_deref() == Some("synth") || synth_pos == Some(pos) {
                    "SYNTH"
                } else if message.kind.as_deref() == Some("plan") || plan_pos == Some(pos) {
                    "PLAN"
                } else {
                    "REPLY"
                };
                cards.push(ArtifactCard {
                    kind,
                    at: message.at.clone(),
                    owner,
                    preview: summarize_text_preview(message.text.as_str(), preview_chars),
                    reference: ArtifactRef::PersistedMessage {
                        message: (*message).clone(),
                    },
                });
            }
        }
    }
    for patch in patches.into_iter().rev() {
        cards.push(ArtifactCard {
            kind: "PATCH",
            at: if patch.status.trim().is_empty() {
                "--".into()
            } else {
                patch.status.clone()
            },
            owner: patch.agent_id.clone(),
            preview: patch.title.clone(),
            reference: ArtifactRef::PersistedPatch {
                path: Some(format!(
                    ".nit/agents/runs/{mission_id}/patches/{}.diff",
                    sanitize_artifact_path_segment(patch.id.as_str())
                )),
                patch,
            },
        });
    }
    for item in evidence.into_iter().rev() {
        let owner = item.agent_id.as_deref().unwrap_or("system").to_string();
        cards.push(ArtifactCard {
            kind: "EVIDENCE",
            at: String::new(),
            owner,
            preview: item.title.clone(),
            reference: ArtifactRef::PersistedEvidence { item },
        });
    }
    cards
}

fn build_persisted_ad_hoc_cards(
    run: &PersistedArtifactsRun,
    agent_id: &str,
    preview_chars: usize,
) -> Vec<ArtifactCard> {
    // Group messages by prompt.
    // Two-pass grouping: assign each reply to its nearest preceding prompt.
    let filtered: Vec<&AgentMessage> = run
        .messages
        .iter()
        .filter(|m| {
            m.mission_id.is_none()
                && (m.agent_id.is_none() || m.agent_id.as_deref() == Some(agent_id))
        })
        .collect();
    let prompt_positions: Vec<usize> = filtered
        .iter()
        .enumerate()
        .filter_map(|(i, m)| m.agent_id.is_none().then_some(i))
        .collect();
    let reply_positions: Vec<usize> = filtered
        .iter()
        .enumerate()
        .filter_map(|(i, m)| m.agent_id.is_some().then_some(i))
        .collect();
    let groups = group_replies_under_prompts(&prompt_positions, &reply_positions);

    let mut patches = run
        .patches
        .iter()
        .filter(|patch| patch.mission_id.is_none() && patch.agent_id == agent_id)
        .cloned()
        .collect::<Vec<_>>();
    let mut evidence = run
        .evidence
        .iter()
        .filter(|item| item.mission_id.is_none() && item.agent_id.as_deref() == Some(agent_id))
        .cloned()
        .collect::<Vec<_>>();

    if patches.len() > ARTIFACT_CARD_LIMIT_PATCHES {
        patches = patches.split_off(patches.len() - ARTIFACT_CARD_LIMIT_PATCHES);
    }
    if evidence.len() > ARTIFACT_CARD_LIMIT_EVIDENCE {
        evidence = evidence.split_off(evidence.len() - ARTIFACT_CARD_LIMIT_EVIDENCE);
    }

    let mut cards = Vec::new();
    for (prompt_pos, mut child_positions) in groups {
        if let Some(pos) = prompt_pos {
            if let Some(message) = filtered.get(pos) {
                cards.push(ArtifactCard {
                    kind: "PROMPT",
                    at: message.at.clone(),
                    owner: "You".into(),
                    preview: summarize_text_preview(message.text.as_str(), preview_chars),
                    reference: ArtifactRef::PersistedMessage {
                        message: (*message).clone(),
                    },
                });
            }
        }
        if child_positions.len() > ARTIFACT_CARD_LIMIT_REPLIES {
            child_positions =
                child_positions.split_off(child_positions.len() - ARTIFACT_CARD_LIMIT_REPLIES);
        }
        for pos in child_positions {
            if let Some(message) = filtered.get(pos) {
                let kind = if message.kind.as_deref() == Some("synth") {
                    "SYNTH"
                } else {
                    "REPLY"
                };
                cards.push(ArtifactCard {
                    kind,
                    at: message.at.clone(),
                    owner: agent_id.into(),
                    preview: summarize_text_preview(message.text.as_str(), preview_chars),
                    reference: ArtifactRef::PersistedMessage {
                        message: (*message).clone(),
                    },
                });
            }
        }
    }
    for patch in patches.into_iter().rev() {
        cards.push(ArtifactCard {
            kind: "PATCH",
            at: if patch.status.trim().is_empty() {
                "--".into()
            } else {
                patch.status.clone()
            },
            owner: patch.agent_id.clone(),
            preview: patch.title.clone(),
            reference: ArtifactRef::PersistedPatch {
                path: Some(format!(
                    "{}patches/{}.diff",
                    persisted_ad_hoc_root(agent_id),
                    sanitize_artifact_path_segment(patch.id.as_str())
                )),
                patch,
            },
        });
    }
    for item in evidence.into_iter().rev() {
        let owner = item.agent_id.as_deref().unwrap_or("system").to_string();
        cards.push(ArtifactCard {
            kind: "EVIDENCE",
            at: String::new(),
            owner,
            preview: item.title.clone(),
            reference: ArtifactRef::PersistedEvidence { item },
        });
    }
    cards
}

fn swarm_verify_status(view: &SwarmPersistenceView) -> &'static str {
    if let Some(report) = view.gate_report.as_ref() {
        if report.overall_ok {
            "PASS"
        } else {
            "FAIL"
        }
    } else if view.gate_bundle.is_some() {
        "PENDING"
    } else {
        "--"
    }
}

fn append_swarm_summary_lines(out: &mut Vec<String>, view: &SwarmPersistenceView, width: usize) {
    let parsed_tasks = view
        .tasks
        .iter()
        .filter(|task| task.artifacts.as_ref().is_some_and(task_artifacts_present))
        .count();
    let missing_tasks = view
        .tasks
        .iter()
        .filter(|task| task.expected_artifacts_missing)
        .count();
    let output_tasks = view.tasks.iter().filter(|task| task.output_present).count();
    let total_files = view
        .tasks
        .iter()
        .map(|task| {
            task.artifacts
                .as_ref()
                .map(|artifacts| artifacts.files.len())
                .unwrap_or(0)
        })
        .sum::<usize>();
    let total_commands = view
        .tasks
        .iter()
        .map(|task| {
            task.artifacts
                .as_ref()
                .map(|artifacts| artifacts.commands.len())
                .unwrap_or(0)
        })
        .sum::<usize>();
    let verify_status = swarm_verify_status(view);

    out.extend(dag_kv_block_lines(
        &[
            ("Mission", view.mission_id.clone()),
            ("Template", view.template.clone()),
            ("Phase", view.phase.clone()),
        ],
        width,
    ));
    out.extend(dag_kv_block_lines(
        &[
            ("Tasks", view.tasks.len().to_string()),
            ("Parsed", parsed_tasks.to_string()),
            ("Missing", missing_tasks.to_string()),
            ("Outputs", output_tasks.to_string()),
            ("Files", total_files.to_string()),
            ("Cmds", total_commands.to_string()),
            ("Verify", verify_status.to_string()),
        ],
        width,
    ));
    out.extend(dag_kv_block_lines(
        &[("Root", format!(".nit/swarm/{}/", view.mission_id))],
        width,
    ));
}

#[cfg(test)]
fn append_swarm_artifact_lines(out: &mut Vec<String>, view: &SwarmPersistenceView, width: usize) {
    append_swarm_summary_lines(out, view, width);
    if view.report_output.is_some() {
        out.push(String::new());
        append_swarm_report_detail_lines(out, view, width);
    }
    out.push(String::new());
    append_swarm_verify_detail_lines(out, view, width);
    for task in view.tasks.iter() {
        out.push(String::new());
        append_swarm_task_detail_lines(out, view, task, width);
    }
}

fn _build_swarm_cards(view: &SwarmPersistenceView, preview_chars: usize) -> Vec<ArtifactCard> {
    let mut cards = Vec::new();
    if let Some(report_output) = view.report_output.as_deref() {
        cards.push(ArtifactCard {
            kind: "REPORT",
            at: view.report_status.clone().unwrap_or_else(|| "FINAL".into()),
            owner: view
                .report_agent_id
                .clone()
                .unwrap_or_else(|| "planner".into()),
            preview: summarize_text_preview(report_output, preview_chars),
            reference: ArtifactRef::SwarmReport {
                mission_id: view.mission_id.clone(),
            },
        });
    }
    if view.gate_bundle.is_some() || view.gate_report.is_some() || view.gate_output.is_some() {
        let status = swarm_verify_status(view);
        let preview = if let Some(report) = view.gate_report.as_ref() {
            let failures = report
                .gates
                .iter()
                .filter(|gate| gate_report_status_label(gate).eq_ignore_ascii_case("FAIL"))
                .count();
            if failures > 0 {
                format!("{failures} failing gate(s)")
            } else {
                "all gates passed".into()
            }
        } else if view.gate_bundle.is_some() {
            "gate bundle selected; report pending".into()
        } else {
            String::new()
        };
        cards.push(ArtifactCard {
            kind: "VERIFY",
            at: status.into(),
            owner: view.gate_bundle.clone().unwrap_or_else(|| "gates".into()),
            preview: summarize_text_preview(preview.as_str(), preview_chars),
            reference: ArtifactRef::SwarmVerify {
                mission_id: view.mission_id.clone(),
            },
        });
    }

    let task_rows = view
        .tasks
        .iter()
        .filter(|task| {
            task.artifacts.as_ref().is_some_and(task_artifacts_present)
                || task.expected_artifacts_missing
                || task.output_present
        })
        .collect::<Vec<_>>();
    for task in task_rows {
        let mut preview = format!("{}: {}", task.id, task.title);
        if task.expected_artifacts_missing {
            preview.push_str(" (missing artifacts)");
        } else if let Some(artifacts) = task.artifacts.as_ref() {
            if let Some(summary) = artifacts.summary.as_deref() {
                preview = format!("{}: {}", task.id, summary);
            }
        }
        cards.push(ArtifactCard {
            kind: "TASK",
            at: task.state.clone(),
            owner: task.agent_id.clone(),
            preview: summarize_text_preview(preview.as_str(), preview_chars),
            reference: ArtifactRef::SwarmTask {
                mission_id: view.mission_id.clone(),
                task_id: task.id.clone(),
            },
        });
    }
    cards
}

fn append_swarm_report_detail_lines(
    out: &mut Vec<String>,
    view: &SwarmPersistenceView,
    width: usize,
) {
    out.extend(dag_kv_block_lines(
        &[
            (
                "Agent",
                view.report_agent_id
                    .clone()
                    .unwrap_or_else(|| "planner".into()),
            ),
            (
                "Status",
                view.report_status.clone().unwrap_or_else(|| "FINAL".into()),
            ),
            (
                "Artifact",
                format!(".nit/swarm/{}/report/final.md", view.mission_id),
            ),
        ],
        width,
    ));
    if let Some(output) = view.report_output.as_deref() {
        let max_lines = 120usize;
        let total_lines = output.lines().count();
        let excerpt = output
            .lines()
            .take(max_lines)
            .collect::<Vec<_>>()
            .join("\n");
        out.push(String::new());
        out.push(" Document (excerpt)".into());
        for line in wrap_cell_text(excerpt.as_str(), width.saturating_sub(1)) {
            out.push(format!(" {line}"));
        }
        if total_lines > max_lines {
            out.push(format!(
                " (report truncated; showing first {max_lines} lines)"
            ));
        }
    }
}

fn append_swarm_task_detail_lines(
    out: &mut Vec<String>,
    view: &SwarmPersistenceView,
    task: &crate::swarm::SwarmTaskPersistenceView,
    width: usize,
) {
    let role = task
        .role
        .as_deref()
        .map(table_role_label)
        .unwrap_or_else(|| "-".into());
    push_wrapped_detail(
        out,
        "task",
        &format!("{} [{}] {}", task.id, task.state, task.title),
        width,
    );
    push_wrapped_detail(
        out,
        "meta",
        &format!(
            "agent={} role={} writes={} output={}",
            task.agent_id,
            role,
            if task.writes { "yes" } else { "no" },
            if task.output_present { "yes" } else { "no" }
        ),
        width,
    );
    if !task.deps.is_empty() {
        push_wrapped_detail(out, "deps", task.deps.join(", ").as_str(), width);
    }
    if !task.blocked_on.is_empty() {
        push_wrapped_detail(
            out,
            "blocked_on",
            task.blocked_on.join(", ").as_str(),
            width,
        );
    }
    if !task.expected_artifacts.is_empty() {
        push_wrapped_detail(
            out,
            "expected",
            task.expected_artifacts.join(", ").as_str(),
            width,
        );
    }
    if task.expected_artifacts_missing {
        push_wrapped_detail(
            out,
            "status",
            "expected artifacts but no parseable swarm_artifacts JSON block was captured",
            width,
        );
    }
    if let Some(done_when) = task.done_when.as_deref() {
        push_wrapped_detail(out, "done_when", done_when, width);
    }
    if let Some(artifacts) = task.artifacts.as_ref() {
        append_swarm_task_artifacts_detail_lines(out, view, task, artifacts, width);
    }
    if task.output_present {
        push_wrapped_detail(
            out,
            "output",
            &format!(
                ".nit/swarm/{}/tasks/{}/output.md",
                view.mission_id,
                sanitize_artifact_path_segment(task.id.as_str())
            ),
            width,
        );
        if let Some(output) = task.output.as_deref() {
            let max_lines = 120usize;
            let total_lines = output.lines().count();
            let excerpt = output
                .lines()
                .take(max_lines)
                .collect::<Vec<_>>()
                .join("\n");
            out.push(String::new());
            out.push(" Output (excerpt)".into());
            for line in wrap_cell_text(excerpt.as_str(), width.saturating_sub(1)) {
                out.push(format!(" {line}"));
            }
            if total_lines > max_lines {
                out.push(format!(
                    " (output truncated; showing first {max_lines} lines)"
                ));
            }
        }
    }
}

fn append_swarm_task_artifacts_detail_lines(
    out: &mut Vec<String>,
    view: &SwarmPersistenceView,
    task: &crate::swarm::SwarmTaskPersistenceView,
    artifacts: &SwarmTaskArtifacts,
    width: usize,
) {
    if let Some(summary) = artifacts.summary.as_deref() {
        push_wrapped_detail(out, "summary", summary, width);
    }
    if !artifacts.files.is_empty() {
        let files = summarize_items(
            artifacts
                .files
                .iter()
                .map(|entry| match entry.notes.as_deref() {
                    Some(notes) if !notes.trim().is_empty() => {
                        format!("{} ({})", entry.path, notes.trim())
                    }
                    _ => entry.path.clone(),
                })
                .collect(),
            12,
        );
        push_wrapped_detail(out, "files", files.as_str(), width);
    }
    if !artifacts.diffs.is_empty() {
        let diffs = summarize_items(
            artifacts
                .diffs
                .iter()
                .map(|entry| match entry.path.as_deref() {
                    Some(path) if !path.trim().is_empty() => {
                        format!("{} ({})", entry.summary, path.trim())
                    }
                    _ => entry.summary.clone(),
                })
                .collect(),
            12,
        );
        push_wrapped_detail(out, "diffs", diffs.as_str(), width);
    }
    if !artifacts.commands.is_empty() {
        let commands = summarize_items(
            artifacts
                .commands
                .iter()
                .map(|entry| match entry.purpose.as_deref() {
                    Some(purpose) if !purpose.trim().is_empty() => {
                        format!("{} ({})", entry.cmd, purpose.trim())
                    }
                    _ => entry.cmd.clone(),
                })
                .collect(),
            10,
        );
        push_wrapped_detail(out, "commands", commands.as_str(), width);
    }
    if !artifacts.risks.is_empty() {
        let risks = summarize_items(
            artifacts
                .risks
                .iter()
                .map(|entry| {
                    let level = entry
                        .level
                        .as_deref()
                        .map(str::trim)
                        .filter(|level| !level.is_empty())
                        .map(|level| format!("[{level}] "))
                        .unwrap_or_default();
                    match entry.mitigation.as_deref() {
                        Some(mitigation) if !mitigation.trim().is_empty() => {
                            format!("{}{} -> {}", level, entry.item, mitigation.trim())
                        }
                        _ => format!("{}{}", level, entry.item),
                    }
                })
                .collect(),
            10,
        );
        push_wrapped_detail(out, "risks", risks.as_str(), width);
    }
    if !artifacts.notes.is_empty() {
        let notes = summarize_items(artifacts.notes.clone(), 10);
        push_wrapped_detail(out, "notes", notes.as_str(), width);
    }
    push_wrapped_detail(
        out,
        "artifact",
        &format!(
            ".nit/swarm/{}/tasks/{}/artifacts.json",
            view.mission_id,
            sanitize_artifact_path_segment(task.id.as_str())
        ),
        width,
    );
}

fn append_swarm_verify_detail_lines(
    out: &mut Vec<String>,
    view: &SwarmPersistenceView,
    width: usize,
) {
    let status = swarm_verify_status(view);
    out.extend(dag_kv_block_lines(
        &[
            (
                "Bundle",
                view.gate_bundle.clone().unwrap_or_else(|| "none".into()),
            ),
            ("Status", status.to_string()),
            (
                "Artifact",
                format!(".nit/swarm/{}/gates/verify.md", view.mission_id),
            ),
        ],
        width,
    ));
    if let Some(report) = view.gate_report.as_ref() {
        for gate in report.gates.iter() {
            push_wrapped_detail(
                out,
                "gate",
                &format!(
                    "{} [{}] {}",
                    gate.name,
                    gate_report_status_label(gate),
                    gate.command
                ),
                width,
            );
            if let Some(notes) = gate.notes.as_deref() {
                push_wrapped_detail(out, "notes", notes, width);
            }
        }
        push_wrapped_detail(
            out,
            "report",
            &format!(".nit/swarm/{}/gates/report.json", view.mission_id),
            width,
        );
    }
    if let Some(output) = view.gate_output.as_deref() {
        push_wrapped_detail(
            out,
            "output_path",
            &format!(".nit/swarm/{}/gates/output.txt", view.mission_id),
            width,
        );
        let max_lines = 120usize;
        let total_lines = output.lines().count();
        let excerpt = output
            .lines()
            .take(max_lines)
            .collect::<Vec<_>>()
            .join("\n");
        out.push(String::new());
        out.push(" Output (excerpt)".into());
        for line in wrap_cell_text(excerpt.as_str(), width.saturating_sub(1)) {
            out.push(format!(" {line}"));
        }
        if total_lines > max_lines {
            out.push(format!(
                " (output truncated; showing first {max_lines} lines)"
            ));
        }
    }
}

fn append_mission_provenance_lines(
    out: &mut Vec<String>,
    state: &AppState,
    mission_id: &str,
    width: usize,
) {
    let Some(mission) = state
        .agents
        .missions
        .iter()
        .find(|mission| mission.id == mission_id)
    else {
        out.push(format!(" Mission {mission_id} not found."));
        return;
    };

    let live_messages = state
        .agents
        .messages
        .iter()
        .filter(|message| message.mission_id.as_deref() == Some(mission_id))
        .collect::<Vec<_>>();
    let live_agent_messages = live_messages
        .iter()
        .copied()
        .filter(|message| message.agent_id.is_some())
        .collect::<Vec<_>>();
    let live_patches = state
        .agents
        .patches
        .iter()
        .filter(|patch| patch.mission_id.as_deref() == Some(mission_id))
        .collect::<Vec<_>>();
    let live_evidence = state
        .agents
        .evidence
        .iter()
        .filter(|item| item.mission_id.as_deref() == Some(mission_id))
        .collect::<Vec<_>>();
    let selected_archived =
        selected_archived_run_for_current_context(state).and_then(|(entry, run)| {
            let has_mission_data = run
                .messages
                .iter()
                .any(|message| message.mission_id.as_deref() == Some(mission_id))
                || run
                    .patches
                    .iter()
                    .any(|patch| patch.mission_id.as_deref() == Some(mission_id))
                || run
                    .evidence
                    .iter()
                    .any(|item| item.mission_id.as_deref() == Some(mission_id));
            has_mission_data.then_some((entry, run))
        });
    let archived_messages = selected_archived
        .as_ref()
        .map(|(_, run)| {
            run.messages
                .iter()
                .filter(|message| message.mission_id.as_deref() == Some(mission_id))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let archived_agent_messages = archived_messages
        .iter()
        .copied()
        .filter(|message| message.agent_id.is_some())
        .collect::<Vec<_>>();
    let archived_patches = selected_archived
        .as_ref()
        .map(|(_, run)| {
            run.patches
                .iter()
                .filter(|patch| patch.mission_id.as_deref() == Some(mission_id))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let archived_evidence = selected_archived
        .as_ref()
        .map(|(_, run)| {
            run.evidence
                .iter()
                .filter(|item| item.mission_id.as_deref() == Some(mission_id))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let persisted = load_persisted_mission_run(state, mission_id);
    let persisted_messages = persisted
        .as_ref()
        .map(|run| {
            run.messages
                .iter()
                .filter(|message| message.mission_id.as_deref() == Some(mission_id))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let persisted_agent_messages = persisted_messages
        .iter()
        .copied()
        .filter(|message| message.agent_id.is_some())
        .collect::<Vec<_>>();
    let persisted_patches = persisted
        .as_ref()
        .map(|run| {
            run.patches
                .iter()
                .filter(|patch| patch.mission_id.as_deref() == Some(mission_id))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let persisted_evidence = persisted
        .as_ref()
        .map(|run| {
            run.evidence
                .iter()
                .filter(|item| item.mission_id.as_deref() == Some(mission_id))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let using_selected_archived = selected_archived.is_some();
    let using_persisted = !using_selected_archived
        && live_messages.is_empty()
        && live_patches.is_empty()
        && live_evidence.is_empty();
    let messages = if using_selected_archived {
        archived_messages.len()
    } else if using_persisted {
        persisted_messages.len()
    } else {
        live_messages.len()
    };
    let agent_messages = if using_selected_archived {
        archived_agent_messages.len()
    } else if using_persisted {
        persisted_agent_messages.len()
    } else {
        live_agent_messages.len()
    };
    let patches = if using_selected_archived {
        archived_patches.len()
    } else if using_persisted {
        persisted_patches.len()
    } else {
        live_patches.len()
    };
    let evidence = if using_selected_archived {
        archived_evidence.len()
    } else if using_persisted {
        persisted_evidence.len()
    } else {
        live_evidence.len()
    };
    let thread_id = state
        .agents
        .selected_context_agent()
        .and_then(|agent_id| {
            state
                .agents
                .codex_mission_thread_ids
                .get(mission_id)?
                .get(agent_id)
        })
        .cloned()
        .or_else(|| {
            state
                .agents
                .codex_mission_thread_ids
                .get(mission_id)
                .and_then(|threads| threads.values().next().cloned())
        });
    let thread_id = thread_id.or_else(|| {
        let run = selected_archived
            .as_ref()
            .map(|(_, run)| run)
            .or(persisted.as_ref())?;
        state
            .agents
            .selected_context_agent()
            .and_then(|agent_id| run.codex_thread_ids.as_ref()?.get(agent_id).cloned())
            .or_else(|| {
                run.codex_thread_ids
                    .as_ref()
                    .and_then(|threads| threads.values().next().cloned())
            })
            .or_else(|| run.codex_thread_id.clone())
    });
    let saved_runs = artifacts_history_entries(state).len().saturating_sub(1);
    let source = if let Some((entry, _)) = selected_archived.as_ref() {
        entry.label.clone()
    } else {
        "current / latest saved run".into()
    };

    out.extend(dag_kv_block_lines(
        &[
            ("Mission", mission.id.clone()),
            (
                "Mode",
                if mission.swarm {
                    "swarm".into()
                } else {
                    "single-agent".into()
                },
            ),
            ("Phase", mission.phase.label().into()),
            ("Status", mission.status.clone()),
        ],
        width,
    ));
    out.extend(dag_kv_block_lines(
        &[
            ("Agents", mission.assigned_agents.len().to_string()),
            ("Msgs", messages.to_string()),
            ("Replies", agent_messages.to_string()),
            ("Patches", patches.to_string()),
            ("Evidence", evidence.to_string()),
        ],
        width,
    ));
    out.extend(dag_kv_block_lines(
        &[("SavedRuns", saved_runs.to_string()), ("Source", source)],
        width,
    ));
    push_wrapped_detail(
        out,
        "root",
        &format!(".nit/agents/runs/{mission_id}/"),
        width,
    );
    push_wrapped_detail(
        out,
        "run",
        &format!(".nit/agents/runs/{mission_id}/run.json"),
        width,
    );
    push_wrapped_detail(
        out,
        "thread",
        &format!(".nit/agents/runs/{mission_id}/thread.md"),
        width,
    );
    // Show Claude session ID for the selected agent in this mission.
    let claude_session = state
        .agents
        .selected_context_agent()
        .and_then(|agent_id| {
            state
                .agents
                .claude_mission_session_ids
                .get(mission_id)?
                .get(agent_id)
                .cloned()
        })
        .or_else(|| {
            state
                .agents
                .claude_mission_session_ids
                .get(mission_id)
                .and_then(|sessions| sessions.values().next().cloned())
        });
    if let Some(session_id) = claude_session.as_deref() {
        push_wrapped_detail(out, "claude_session", session_id, width);
    }
    if let Some(thread_id) = thread_id.as_deref() {
        push_wrapped_detail(out, "codex_thread", thread_id, width);
    }
    if !mission.assigned_agents.is_empty() {
        push_wrapped_detail(
            out,
            "assigned",
            mission.assigned_agents.join(", ").as_str(),
            width,
        );
    }
    push_wrapped_detail(
        out,
        "note",
        if using_selected_archived {
            "Showing an archived saved run from disk. Press R in ARTIFACTS to switch back to current/latest."
        } else if mission.swarm {
            "Showing mission provenance fallback because no live swarm artifact view was available."
        } else if using_persisted {
            "Showing saved single-agent mission provenance from disk because the live thread is empty."
        } else {
            "Showing single-agent mission provenance from the saved run and thread."
        },
        width,
    );
    push_wrapped_detail(out, "history", "Press R to browse saved runs.", width);
}

fn append_ad_hoc_agent_lines(
    out: &mut Vec<String>,
    state: &AppState,
    agent_id: &str,
    width: usize,
) {
    let live_messages = state
        .agents
        .messages
        .iter()
        .filter(|message| {
            message.mission_id.is_none()
                && (message.agent_id.is_none() || message.agent_id.as_deref() == Some(agent_id))
        })
        .collect::<Vec<_>>();
    let live_agent_messages = live_messages
        .iter()
        .copied()
        .filter(|message| message.agent_id.as_deref() == Some(agent_id))
        .collect::<Vec<_>>();
    let live_patches = state
        .agents
        .patches
        .iter()
        .filter(|patch| patch.mission_id.is_none() && patch.agent_id == agent_id)
        .collect::<Vec<_>>();
    let live_evidence = state
        .agents
        .evidence
        .iter()
        .filter(|item| item.mission_id.is_none() && item.agent_id.as_deref() == Some(agent_id))
        .collect::<Vec<_>>();
    let selected_archived =
        selected_archived_run_for_current_context(state).and_then(|(entry, run)| {
            let has_agent_data = run.messages.iter().any(|message| {
                message.mission_id.is_none()
                    && (message.agent_id.is_none() || message.agent_id.as_deref() == Some(agent_id))
            }) || run
                .patches
                .iter()
                .any(|patch| patch.mission_id.is_none() && patch.agent_id == agent_id)
                || run.evidence.iter().any(|item| {
                    item.mission_id.is_none() && item.agent_id.as_deref() == Some(agent_id)
                });
            has_agent_data.then_some((entry, run))
        });
    let archived_messages = selected_archived
        .as_ref()
        .map(|(_, run)| {
            run.messages
                .iter()
                .filter(|message| {
                    message.mission_id.is_none()
                        && (message.agent_id.is_none()
                            || message.agent_id.as_deref() == Some(agent_id))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let archived_agent_messages = archived_messages
        .iter()
        .copied()
        .filter(|message| message.agent_id.as_deref() == Some(agent_id))
        .collect::<Vec<_>>();
    let archived_patches = selected_archived
        .as_ref()
        .map(|(_, run)| {
            run.patches
                .iter()
                .filter(|patch| patch.mission_id.is_none() && patch.agent_id == agent_id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let persisted = load_persisted_ad_hoc_run(state, agent_id);
    let persisted_messages = persisted
        .as_ref()
        .map(|run| {
            run.messages
                .iter()
                .filter(|message| {
                    message.mission_id.is_none()
                        && (message.agent_id.is_none()
                            || message.agent_id.as_deref() == Some(agent_id))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let persisted_agent_messages = persisted_messages
        .iter()
        .copied()
        .filter(|message| message.agent_id.as_deref() == Some(agent_id))
        .collect::<Vec<_>>();
    let persisted_patches = persisted
        .as_ref()
        .map(|run| {
            run.patches
                .iter()
                .filter(|patch| patch.mission_id.is_none() && patch.agent_id == agent_id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let _persisted_evidence = persisted
        .as_ref()
        .map(|run| {
            run.evidence
                .iter()
                .filter(|item| {
                    item.mission_id.is_none() && item.agent_id.as_deref() == Some(agent_id)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let using_selected_archived = selected_archived.is_some();
    let using_persisted = !using_selected_archived
        && live_messages.is_empty()
        && live_patches.is_empty()
        && live_evidence.is_empty();
    let messages = if using_selected_archived {
        archived_messages.len()
    } else if using_persisted {
        persisted_messages.len()
    } else {
        live_messages.len()
    };
    let agent_messages = if using_selected_archived {
        archived_agent_messages.len()
    } else if using_persisted {
        persisted_agent_messages.len()
    } else {
        live_agent_messages.len()
    };
    let patches = if using_selected_archived {
        archived_patches.len()
    } else if using_persisted {
        persisted_patches.len()
    } else {
        live_patches.len()
    };
    let thread_present = state.agents.codex_thread_ids.contains_key(agent_id)
        || state.agents.claude_session_ids.contains_key(agent_id)
        || selected_archived
            .as_ref()
            .and_then(|(_, run)| run.codex_thread_id.as_ref())
            .is_some()
        || persisted
            .as_ref()
            .and_then(|run| run.codex_thread_id.as_ref())
            .is_some();
    let saved_runs = artifacts_history_entries(state).len().saturating_sub(1);
    let source = if let Some((entry, _)) = selected_archived.as_ref() {
        entry.label.clone()
    } else {
        "current / latest saved run".into()
    };

    out.extend(dag_kv_block_lines(
        &[
            ("Agent", agent_id.to_string()),
            ("Context", "ad-hoc".into()),
        ],
        width,
    ));
    out.extend(dag_kv_block_lines(
        &[
            ("Msgs", messages.to_string()),
            ("Replies", agent_messages.to_string()),
            ("Patches", patches.to_string()),
            (
                "Thread",
                if thread_present {
                    "yes".into()
                } else {
                    "no".into()
                },
            ),
        ],
        width,
    ));
    out.extend(dag_kv_block_lines(
        &[("SavedRuns", saved_runs.to_string()), ("Source", source)],
        width,
    ));
    push_wrapped_detail(out, "root", persisted_ad_hoc_root(agent_id).as_str(), width);
    push_wrapped_detail(
        out,
        "run",
        &format!("{}run.json", persisted_ad_hoc_root(agent_id)),
        width,
    );
    push_wrapped_detail(
        out,
        "thread",
        &format!("{}thread.md", persisted_ad_hoc_root(agent_id)),
        width,
    );
    push_wrapped_detail(
        out,
        "note",
        if using_selected_archived {
            "Showing an archived ad-hoc saved run from disk. Press R in ARTIFACTS to switch back to current/latest."
        } else if using_persisted {
            "No mission is selected. Showing saved ad-hoc artifacts from disk because the live thread is empty."
        } else {
            "No mission is selected. This view is built from the live ad-hoc thread for the selected agent. Select a mission in MISSIONS to see mission artifacts."
        },
        width,
    );
    push_wrapped_detail(out, "history", "Press R to browse saved runs.", width);
    if let Some(session_id) = state.agents.claude_session_ids.get(agent_id) {
        push_wrapped_detail(out, "claude_session", session_id.as_str(), width);
    }
    if let Some(thread_id) = state
        .agents
        .codex_thread_ids
        .get(agent_id)
        .cloned()
        .or_else(|| {
            selected_archived
                .as_ref()
                .and_then(|(_, run)| run.codex_thread_id.clone())
        })
        .or_else(|| persisted.and_then(|run| run.codex_thread_id))
    {
        push_wrapped_detail(out, "codex_thread", thread_id.as_str(), width);
    }
}

pub fn artifact_cards_for_context(
    state: &AppState,
    swarm: Option<&SwarmRuntime>,
    preview_chars: usize,
) -> Vec<ArtifactCard> {
    // If the user explicitly selected an archived run in the history browser, show only that.
    if let Some((_, run)) = selected_archived_run_for_current_context(state) {
        if let Some(mission_id) = state.agents.selected_context_mission() {
            return build_persisted_mission_cards(&run, mission_id, preview_chars);
        }
        if let Some(agent_id) = state.agents.selected_context_agent() {
            return build_persisted_ad_hoc_cards(&run, agent_id, preview_chars);
        }
    }

    if let Some(mission_id) = state.agents.selected_context_mission() {
        if let Some(view) = swarm.and_then(|runtime| runtime.swarm_persistence(mission_id)) {
            // Use the same prompt→reply tree as regular missions so the
            // graph shows root prompts with agent reply branches.
            let mut cards = build_mission_cards(state, mission_id, preview_chars);

            // Upgrade reply cards: match against swarm task outputs (→ TASK),
            // label only the very first non-task reply as PLAN (the actual
            // swarm plan), all others stay REPLY.
            {
                let mut plan_found = false;
                for card in cards.iter_mut() {
                    if card.kind != "REPLY" {
                        continue;
                    }
                    if let ArtifactRef::Message { idx } = &card.reference {
                        if let Some(msg) = state.agents.messages.get(*idx) {
                            if let Some(agent_id) = msg.agent_id.as_deref() {
                                if let Some(task) = view.tasks.iter().rev().find(|t| {
                                    t.agent_id == agent_id
                                        && t.output.as_deref() == Some(msg.text.as_str())
                                }) {
                                    card.kind = "TASK";
                                    card.at =
                                        task.role.as_deref().unwrap_or(&task.state).to_string();
                                    card.reference = ArtifactRef::SwarmTask {
                                        mission_id: view.mission_id.clone(),
                                        task_id: task.id.clone(),
                                    };
                                } else if !plan_found {
                                    card.kind = "PLAN";
                                    card.at = "planner".into();
                                    plan_found = true;
                                }
                                // All other non-task replies stay as "REPLY".
                            }
                        }
                    }
                }
            }

            // Append REPORT and VERIFY as global cards (outside the tree).
            if let Some(report_output) = view.report_output.as_deref() {
                cards.push(ArtifactCard {
                    kind: "REPORT",
                    at: view.report_status.clone().unwrap_or_else(|| "FINAL".into()),
                    owner: view
                        .report_agent_id
                        .clone()
                        .unwrap_or_else(|| "planner".into()),
                    preview: summarize_text_preview(report_output, preview_chars),
                    reference: ArtifactRef::SwarmReport {
                        mission_id: view.mission_id.clone(),
                    },
                });
            }
            if view.gate_bundle.is_some()
                || view.gate_report.is_some()
                || view.gate_output.is_some()
            {
                let status = swarm_verify_status(&view);
                let preview = if let Some(report) = view.gate_report.as_ref() {
                    let failures = report
                        .gates
                        .iter()
                        .filter(|gate| gate_report_status_label(gate).eq_ignore_ascii_case("FAIL"))
                        .count();
                    if failures > 0 {
                        format!("{failures} failing gate(s)")
                    } else {
                        "all gates passed".into()
                    }
                } else if view.gate_bundle.is_some() {
                    "gate bundle selected; report pending".into()
                } else {
                    String::new()
                };
                cards.insert(
                    0,
                    ArtifactCard {
                        kind: "VERIFY",
                        at: status.into(),
                        owner: view.gate_bundle.clone().unwrap_or_else(|| "gates".into()),
                        preview: summarize_text_preview(preview.as_str(), preview_chars),
                        reference: ArtifactRef::SwarmVerify {
                            mission_id: view.mission_id.clone(),
                        },
                    },
                );
            }
            return cards;
        }
        // Prefer live cards (correct tree order from grouping). Fall back to persisted.
        let live = build_mission_cards(state, mission_id, preview_chars);
        if !live.is_empty() {
            return live;
        }
        if let Some(run) = load_persisted_mission_run(state, mission_id) {
            return build_persisted_mission_cards(&run, mission_id, preview_chars);
        }
        return live;
    }

    if let Some(agent_id) = state.agents.selected_context_agent() {
        let live = build_ad_hoc_cards(state, agent_id, preview_chars);
        if !live.is_empty() {
            return live;
        }
        if let Some(run) = load_persisted_ad_hoc_run(state, agent_id) {
            return build_persisted_ad_hoc_cards(&run, agent_id, preview_chars);
        }
        return live;
    }

    Vec::new()
}

fn artifacts_lines(state: &AppState, swarm: Option<&SwarmRuntime>, width: usize) -> Vec<String> {
    let width = width.max(32);
    let mut out = vec![" ARTIFACTS".into(), "─".repeat(width.min(240))];

    let widths = artifact_list_widths(width);
    let preview_chars = widths
        .get(3)
        .copied()
        .unwrap_or(120)
        .saturating_sub(1)
        .max(10);

    let cards = if let Some(mission_id) = state.agents.selected_context_mission() {
        if let Some(view) = swarm.and_then(|runtime| runtime.swarm_persistence(mission_id)) {
            append_swarm_summary_lines(&mut out, &view, width);
        } else {
            append_mission_provenance_lines(&mut out, state, mission_id, width);
        }
        artifact_cards_for_context(state, swarm, preview_chars)
    } else if let Some(agent_id) = state.agents.selected_context_agent() {
        append_ad_hoc_agent_lines(&mut out, state, agent_id, width);
        artifact_cards_for_context(state, swarm, preview_chars)
    } else {
        out.push(" No mission or agent selected.".into());
        return out;
    };

    out.push("─".repeat(width.min(240)));
    out.push(" Items".into());
    if cards.is_empty() {
        out.push(" No artifacts captured yet.".into());
        return out;
    }
    let selected_idx = state
        .agents
        .artifacts_selected
        .min(cards.len().saturating_sub(1));

    // Render in tree style: prompts are roots, everything else is a child.
    // REPORT/VERIFY are global (outside the tree). Prompt groups are collapsible.
    let has_prompt = cards.iter().any(|c| c.kind == "PROMPT");
    let is_global = |kind: &str| matches!(kind, "REPORT" | "VERIFY");
    for (idx, card) in cards.iter().enumerate() {
        // Collapsed is per-iteration: only the current PROMPT card can set it.
        let mut collapsed = false;
        if card.kind == "PROMPT" {
            collapsed = state.agents.artifacts_collapsed_prompts.contains(&idx);
            // Count children for the collapse indicator.
            let child_count = cards[idx + 1..]
                .iter()
                .take_while(|c| c.kind != "PROMPT" && !is_global(c.kind))
                .count();
            let suffix = if collapsed {
                format!(" [{child_count} hidden]")
            } else {
                String::new()
            };
            let mut row = artifact_card_row(card, widths.as_slice(), idx == selected_idx, false);
            if !suffix.is_empty() {
                // Trim trailing spaces and append.
                row = row.trim_end().to_string();
                row.push_str(&suffix);
            }
            out.push(row);
        } else if is_global(card.kind) {
            // Global cards are always visible, not under any prompt.
            out.push(artifact_card_row(
                card,
                widths.as_slice(),
                idx == selected_idx,
                false,
            ));
        } else {
            // Child card — skip if parent prompt is collapsed.
            if collapsed {
                continue;
            }
            let is_child = has_prompt;
            out.push(artifact_card_row(
                card,
                widths.as_slice(),
                idx == selected_idx,
                is_child,
            ));
        }
    }

    out
}

/// Check if a line is an artifact card row.
/// Root format: `{' '|'>'}{→|➜|>}{' '}...`
/// Child format: `{' '|'>'}{' '}{' '}{↳|➜}{' '}...`
fn is_artifacts_card_row(line: &str) -> bool {
    is_artifacts_root_card_row(line) || is_artifacts_child_card_row(line)
}

fn is_artifacts_root_card_row(line: &str) -> bool {
    let mut chars = line.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first != ' ' && first != '>' {
        return false;
    }
    let Some(second) = chars.next() else {
        return false;
    };
    if second == PROMPT_GLYPH || second == CURSOR_PRIMARY || second == '>' {
        return chars.next() == Some(' ');
    }
    false
}

fn is_artifacts_child_card_row(line: &str) -> bool {
    let mut chars = line.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first != ' ' && first != '>' {
        return false;
    }
    // Child rows: two spaces then glyph then space.
    if chars.next() != Some(' ') {
        return false;
    }
    if chars.next() != Some(' ') {
        return false;
    }
    let Some(glyph) = chars.next() else {
        return false;
    };
    if glyph == '↳' || glyph == CURSOR_PRIMARY {
        return chars.next() == Some(' ');
    }
    false
}

pub fn artifacts_card_index_for_line(lines: &[String], line_idx: usize) -> Option<usize> {
    let mut card_idx = 0usize;
    for (idx, line) in lines.iter().enumerate() {
        if !is_artifacts_card_row(line) {
            continue;
        }
        if idx == line_idx {
            return Some(card_idx);
        }
        card_idx = card_idx.saturating_add(1);
    }
    None
}

pub fn artifacts_card_line_for_index(lines: &[String], card_idx: usize) -> Option<usize> {
    let mut cursor = 0usize;
    for (idx, line) in lines.iter().enumerate() {
        if !is_artifacts_card_row(line) {
            continue;
        }
        if cursor == card_idx {
            return Some(idx);
        }
        cursor = cursor.saturating_add(1);
    }
    None
}

pub fn artifacts_card_count(lines: &[String]) -> usize {
    lines
        .iter()
        .filter(|line| is_artifacts_card_row(line.as_str()))
        .count()
}

#[derive(Clone, Debug)]
pub enum ArtifactsPopupRef {
    Message {
        idx: usize,
    },
    Patch {
        idx: usize,
    },
    Evidence {
        idx: usize,
    },
    PersistedMessage {
        message: AgentMessage,
    },
    PersistedPatch {
        patch: PersistedPatchRecord,
        path: Option<String>,
    },
    PersistedEvidence {
        item: EvidenceItem,
    },
    SwarmTask {
        mission_id: String,
        task_id: String,
    },
    SwarmReport {
        mission_id: String,
    },
    SwarmVerify {
        mission_id: String,
    },
}

pub fn artifacts_popup_ref(
    state: &AppState,
    swarm: &SwarmRuntime,
    width: usize,
) -> Option<ArtifactsPopupRef> {
    let width = width.max(32);
    let widths = artifact_list_widths(width);
    let preview_chars = widths
        .get(3)
        .copied()
        .unwrap_or(120)
        .saturating_sub(1)
        .max(10);

    let cards = artifact_cards_for_context(state, Some(swarm), preview_chars);

    if cards.is_empty() {
        return None;
    }
    let selected_idx = state
        .agents
        .artifacts_selected
        .min(cards.len().saturating_sub(1));
    let card = &cards[selected_idx];

    match &card.reference {
        ArtifactRef::Message { idx } => Some(ArtifactsPopupRef::Message { idx: *idx }),
        ArtifactRef::Patch { idx } => Some(ArtifactsPopupRef::Patch { idx: *idx }),
        ArtifactRef::Evidence { idx } => Some(ArtifactsPopupRef::Evidence { idx: *idx }),
        ArtifactRef::PersistedMessage { message } => Some(ArtifactsPopupRef::PersistedMessage {
            message: message.clone(),
        }),
        ArtifactRef::PersistedPatch { patch, path } => Some(ArtifactsPopupRef::PersistedPatch {
            patch: patch.clone(),
            path: path.clone(),
        }),
        ArtifactRef::PersistedEvidence { item } => {
            Some(ArtifactsPopupRef::PersistedEvidence { item: item.clone() })
        }
        ArtifactRef::SwarmTask {
            mission_id,
            task_id,
        } => Some(ArtifactsPopupRef::SwarmTask {
            mission_id: mission_id.clone(),
            task_id: task_id.clone(),
        }),
        ArtifactRef::SwarmReport { mission_id } => Some(ArtifactsPopupRef::SwarmReport {
            mission_id: mission_id.clone(),
        }),
        ArtifactRef::SwarmVerify { mission_id } => Some(ArtifactsPopupRef::SwarmVerify {
            mission_id: mission_id.clone(),
        }),
    }
}

/// Returns the agent_id that produced the currently selected artifact, if any.
/// This is used to dispatch from the artifacts popup chat to the correct agent context.
pub fn selected_artifact_agent_id(
    state: &AppState,
    swarm: &SwarmRuntime,
    width: usize,
) -> Option<String> {
    let width = width.max(32);
    let widths = artifact_list_widths(width);
    let preview_chars = widths
        .get(3)
        .copied()
        .unwrap_or(120)
        .saturating_sub(1)
        .max(10);
    let cards = artifact_cards_for_context(state, Some(swarm), preview_chars);
    if cards.is_empty() {
        return None;
    }
    let selected_idx = state
        .agents
        .artifacts_selected
        .min(cards.len().saturating_sub(1));
    let card = &cards[selected_idx];

    // Try to resolve agent_id from the card reference.
    match &card.reference {
        ArtifactRef::Message { idx } => state
            .agents
            .messages
            .get(*idx)
            .and_then(|msg| msg.agent_id.clone()),
        ArtifactRef::SwarmTask { mission_id, .. }
        | ArtifactRef::SwarmReport { mission_id }
        | ArtifactRef::SwarmVerify { mission_id } => {
            // For swarm artifacts, check the card owner field which contains the agent id.
            let owner = card.owner.trim();
            if !owner.is_empty() && state.agents.agents.iter().any(|a| a.id == owner) {
                Some(owner.to_string())
            } else {
                // Fall back to the planner for this mission.
                swarm
                    .session_config(mission_id)
                    .map(|c| c.planner_agent_id.clone())
            }
        }
        _ => None,
    }
}

/// Returns the mission_id associated with the currently selected artifact, if any.
pub fn selected_artifact_mission_id(
    state: &AppState,
    swarm: &SwarmRuntime,
    width: usize,
) -> Option<String> {
    let width = width.max(32);
    let widths = artifact_list_widths(width);
    let preview_chars = widths
        .get(3)
        .copied()
        .unwrap_or(120)
        .saturating_sub(1)
        .max(10);
    let cards = artifact_cards_for_context(state, Some(swarm), preview_chars);
    if cards.is_empty() {
        return None;
    }
    let selected_idx = state
        .agents
        .artifacts_selected
        .min(cards.len().saturating_sub(1));
    let card = &cards[selected_idx];

    match &card.reference {
        ArtifactRef::Message { idx } => state
            .agents
            .messages
            .get(*idx)
            .and_then(|msg| msg.mission_id.clone()),
        ArtifactRef::SwarmTask { mission_id, .. }
        | ArtifactRef::SwarmReport { mission_id }
        | ArtifactRef::SwarmVerify { mission_id } => Some(mission_id.clone()),
        _ => None,
    }
}

/// Returns `true` if the currently selected artifact is a user prompt (not an agent reply).
pub fn is_selected_artifact_prompt(state: &AppState, swarm: &SwarmRuntime, width: usize) -> bool {
    let width = width.max(32);
    let widths = artifact_list_widths(width);
    let preview_chars = widths
        .get(3)
        .copied()
        .unwrap_or(120)
        .saturating_sub(1)
        .max(10);
    let cards = artifact_cards_for_context(state, Some(swarm), preview_chars);
    if cards.is_empty() {
        return false;
    }
    let selected_idx = state
        .agents
        .artifacts_selected
        .min(cards.len().saturating_sub(1));
    cards[selected_idx].kind == "PROMPT"
}

pub fn artifacts_popup_ref_for_message(
    state: &AppState,
    swarm: Option<&SwarmRuntime>,
    width: usize,
    message_idx: usize,
) -> Option<ArtifactsPopupRef> {
    let message = state.agents.messages.get(message_idx)?;
    if let Some(swarm) = swarm {
        if let Some((mission_id, agent_id)) = message
            .mission_id
            .as_deref()
            .zip(message.agent_id.as_deref())
        {
            if let Some(view) = swarm.swarm_persistence(mission_id) {
                if view.report_output.as_deref() == Some(message.text.as_str())
                    && view
                        .report_agent_id
                        .as_deref()
                        .is_none_or(|report_agent_id| report_agent_id == agent_id)
                {
                    return Some(ArtifactsPopupRef::SwarmReport {
                        mission_id: mission_id.to_string(),
                    });
                }

                if let Some(task) = view.tasks.iter().rev().find(|task| {
                    task.agent_id == agent_id
                        && task.output.as_deref() == Some(message.text.as_str())
                }) {
                    return Some(ArtifactsPopupRef::SwarmTask {
                        mission_id: mission_id.to_string(),
                        task_id: task.id.clone(),
                    });
                }

                if view.gate_output.as_deref() == Some(message.text.as_str()) {
                    return Some(ArtifactsPopupRef::SwarmVerify {
                        mission_id: mission_id.to_string(),
                    });
                }
            }
        }
    }

    // If the message has an agent_id it is an agent reply and always viewable as an artifact,
    // even when it doesn't appear in the current artifact card list (e.g. the planner's
    // initial reply in a swarm, which build_swarm_cards doesn't include).
    if message.agent_id.is_some() {
        return Some(ArtifactsPopupRef::Message { idx: message_idx });
    }

    artifacts_card_index_for_message(state, swarm, width, message_idx)
        .map(|_| ArtifactsPopupRef::Message { idx: message_idx })
}

pub fn artifacts_card_index_for_swarm_task(
    state: &AppState,
    swarm: &SwarmRuntime,
    width: usize,
    mission_id: &str,
    task_id: &str,
) -> Option<usize> {
    let width = width.max(32);
    let widths = artifact_list_widths(width);
    let preview_chars = widths
        .get(3)
        .copied()
        .unwrap_or(120)
        .saturating_sub(1)
        .max(10);
    let cards = artifact_cards_for_context(state, Some(swarm), preview_chars);
    cards.iter().position(|card| {
        matches!(
            &card.reference,
            ArtifactRef::SwarmTask {
                mission_id: card_mission_id,
                task_id: card_task_id,
            } if card_mission_id == mission_id && card_task_id == task_id
        )
    })
}

pub fn artifacts_card_index_for_swarm_report(
    state: &AppState,
    swarm: &SwarmRuntime,
    width: usize,
    mission_id: &str,
) -> Option<usize> {
    let width = width.max(32);
    let widths = artifact_list_widths(width);
    let preview_chars = widths
        .get(3)
        .copied()
        .unwrap_or(120)
        .saturating_sub(1)
        .max(10);
    let cards = artifact_cards_for_context(state, Some(swarm), preview_chars);
    cards.iter().position(|card| {
        matches!(
            &card.reference,
            ArtifactRef::SwarmReport {
                mission_id: card_mission_id,
            } if card_mission_id == mission_id
        )
    })
}

pub fn artifacts_card_index_for_swarm_verify(
    state: &AppState,
    swarm: &SwarmRuntime,
    width: usize,
    mission_id: &str,
) -> Option<usize> {
    let width = width.max(32);
    let widths = artifact_list_widths(width);
    let preview_chars = widths
        .get(3)
        .copied()
        .unwrap_or(120)
        .saturating_sub(1)
        .max(10);
    let cards = artifact_cards_for_context(state, Some(swarm), preview_chars);
    cards.iter().position(|card| {
        matches!(
            &card.reference,
            ArtifactRef::SwarmVerify {
                mission_id: card_mission_id,
            } if card_mission_id == mission_id
        )
    })
}

pub fn artifacts_card_index_for_message(
    state: &AppState,
    swarm: Option<&SwarmRuntime>,
    width: usize,
    message_idx: usize,
) -> Option<usize> {
    let width = width.max(32);
    let widths = artifact_list_widths(width);
    let preview_chars = widths
        .get(3)
        .copied()
        .unwrap_or(120)
        .saturating_sub(1)
        .max(10);
    let cards = artifact_cards_for_context(state, swarm, preview_chars);
    cards.iter().position(
        |card| matches!(&card.reference, ArtifactRef::Message { idx } if *idx == message_idx),
    )
}

pub fn artifacts_card_index_for_popup_ref(
    state: &AppState,
    swarm: Option<&SwarmRuntime>,
    width: usize,
    popup_ref: &ArtifactsPopupRef,
) -> Option<usize> {
    match popup_ref {
        ArtifactsPopupRef::Message { idx } => {
            artifacts_card_index_for_message(state, swarm, width, *idx)
        }
        ArtifactsPopupRef::Patch { .. }
        | ArtifactsPopupRef::Evidence { .. }
        | ArtifactsPopupRef::PersistedMessage { .. }
        | ArtifactsPopupRef::PersistedPatch { .. }
        | ArtifactsPopupRef::PersistedEvidence { .. } => None,
        ArtifactsPopupRef::SwarmTask {
            mission_id,
            task_id,
        } => swarm.and_then(|swarm| {
            artifacts_card_index_for_swarm_task(state, swarm, width, mission_id, task_id)
        }),
        ArtifactsPopupRef::SwarmReport { mission_id } => swarm.and_then(|swarm| {
            artifacts_card_index_for_swarm_report(state, swarm, width, mission_id)
        }),
        ArtifactsPopupRef::SwarmVerify { mission_id } => swarm.and_then(|swarm| {
            artifacts_card_index_for_swarm_verify(state, swarm, width, mission_id)
        }),
    }
}

pub fn artifacts_popup_strings(
    state: &AppState,
    swarm: &SwarmRuntime,
    width: usize,
) -> Vec<String> {
    let width = width.max(32);
    let widths = artifact_list_widths(width);
    let preview_chars = widths
        .get(3)
        .copied()
        .unwrap_or(120)
        .saturating_sub(1)
        .max(10);

    let cards = artifact_cards_for_context(state, Some(swarm), preview_chars);

    if cards.is_empty() {
        return vec![" No artifacts captured yet.".into()];
    }
    let selected_idx = state
        .agents
        .artifacts_selected
        .min(cards.len().saturating_sub(1));
    let card = &cards[selected_idx];

    let mut out = vec![format!(
        " {}  {}  {}",
        card.kind,
        card.owner.as_str(),
        if card.at.trim().is_empty() {
            "--"
        } else {
            card.at.as_str()
        }
    )];
    out.push("─".repeat(width.min(240)));

    match &card.reference {
        ArtifactRef::Message { idx } => {
            let Some(message) = state.agents.messages.get(*idx) else {
                out.push(" Message not found.".into());
                return out;
            };
            push_wrapped_detail(&mut out, "at", message.at.as_str(), width);
            if let Some(agent) = message.agent_id.as_deref() {
                push_wrapped_detail(&mut out, "agent", agent, width);
            } else {
                push_wrapped_detail(&mut out, "agent", "You", width);
            }
            if let Some(mission_id) = message.mission_id.as_deref() {
                push_wrapped_detail(&mut out, "mission", mission_id, width);
            } else {
                push_wrapped_detail(&mut out, "mission", "ad-hoc", width);
            }
            out.push("─".repeat(width.min(240)));
            out.push(" Content".into());
            for line in wrap_cell_text(message.text.as_str(), width.saturating_sub(1)) {
                out.push(format!(" {line}"));
            }
        }
        ArtifactRef::PersistedMessage { message } => {
            push_wrapped_detail(&mut out, "at", message.at.as_str(), width);
            if let Some(agent) = message.agent_id.as_deref() {
                push_wrapped_detail(&mut out, "agent", agent, width);
            } else {
                push_wrapped_detail(&mut out, "agent", "You", width);
            }
            if let Some(mission_id) = message.mission_id.as_deref() {
                push_wrapped_detail(&mut out, "mission", mission_id, width);
            } else {
                push_wrapped_detail(&mut out, "mission", "ad-hoc", width);
            }
            out.push("─".repeat(width.min(240)));
            out.push(" Content".into());
            for line in wrap_cell_text(message.text.as_str(), width.saturating_sub(1)) {
                out.push(format!(" {line}"));
            }
        }
        ArtifactRef::Patch { idx } => {
            let Some(patch) = state.agents.patches.get(*idx) else {
                out.push(" Patch not found.".into());
                return out;
            };
            push_wrapped_detail(&mut out, "id", patch.id.as_str(), width);
            push_wrapped_detail(&mut out, "status", patch.status.label(), width);
            push_wrapped_detail(&mut out, "agent", patch.agent_id.as_str(), width);
            if let Some(mission_id) = patch.mission_id.as_deref() {
                push_wrapped_detail(&mut out, "mission", mission_id, width);
                push_wrapped_detail(
                    &mut out,
                    "path",
                    &format!(
                        ".nit/agents/runs/{}/patches/{}.diff",
                        mission_id,
                        sanitize_artifact_path_segment(patch.id.as_str())
                    ),
                    width,
                );
            } else {
                push_wrapped_detail(&mut out, "mission", "ad-hoc", width);
            }
            push_wrapped_detail(&mut out, "title", patch.title.as_str(), width);
            push_wrapped_detail(&mut out, "summary", patch.summary.as_str(), width);

            out.push("─".repeat(width.min(240)));
            out.push(" Diff (excerpt)".into());
            let max_lines = 180usize;
            let total_lines = patch.diff.lines().count();
            let excerpt = patch
                .diff
                .lines()
                .take(max_lines)
                .collect::<Vec<_>>()
                .join("\n");
            for line in wrap_cell_text(excerpt.as_str(), width.saturating_sub(1)) {
                out.push(format!(" {line}"));
            }
            if total_lines > max_lines {
                out.push(format!(
                    " (diff truncated; showing first {max_lines} lines)"
                ));
            }
        }
        ArtifactRef::PersistedPatch { patch, path } => {
            push_wrapped_detail(&mut out, "id", patch.id.as_str(), width);
            push_wrapped_detail(
                &mut out,
                "status",
                if patch.status.trim().is_empty() {
                    "--"
                } else {
                    patch.status.as_str()
                },
                width,
            );
            if !patch.agent_id.trim().is_empty() {
                push_wrapped_detail(&mut out, "agent", patch.agent_id.as_str(), width);
            }
            if let Some(mission_id) = patch.mission_id.as_deref() {
                push_wrapped_detail(&mut out, "mission", mission_id, width);
            } else {
                push_wrapped_detail(&mut out, "mission", "ad-hoc", width);
            }
            if let Some(path) = path.as_deref() {
                push_wrapped_detail(&mut out, "path", path, width);
            }
            if !patch.title.trim().is_empty() {
                push_wrapped_detail(&mut out, "title", patch.title.as_str(), width);
            }
            if !patch.summary.trim().is_empty() {
                push_wrapped_detail(&mut out, "summary", patch.summary.as_str(), width);
            }

            out.push("─".repeat(width.min(240)));
            out.push(" Diff (excerpt)".into());
            let diff = persisted_patch_diff_excerpt(patch, path.as_deref());
            let max_lines = 180usize;
            let total_lines = diff.lines().count();
            let excerpt = diff.lines().take(max_lines).collect::<Vec<_>>().join("\n");
            for line in wrap_cell_text(excerpt.as_str(), width.saturating_sub(1)) {
                out.push(format!(" {line}"));
            }
            if total_lines > max_lines {
                out.push(format!(
                    " (diff truncated; showing first {max_lines} lines)"
                ));
            }
        }
        ArtifactRef::Evidence { idx } => {
            let Some(item) = state.agents.evidence.get(*idx) else {
                out.push(" Evidence item not found.".into());
                return out;
            };
            push_wrapped_detail(&mut out, "id", item.id.as_str(), width);
            if let Some(agent) = item.agent_id.as_deref() {
                push_wrapped_detail(&mut out, "agent", agent, width);
            }
            if let Some(mission_id) = item.mission_id.as_deref() {
                push_wrapped_detail(&mut out, "mission", mission_id, width);
            } else {
                push_wrapped_detail(&mut out, "mission", "ad-hoc", width);
            }
            push_wrapped_detail(&mut out, "title", item.title.as_str(), width);
            push_wrapped_detail(&mut out, "detail", item.detail.as_str(), width);
            if let Some(link) = item.link.as_deref() {
                push_wrapped_detail(&mut out, "link", link, width);
            }
        }
        ArtifactRef::PersistedEvidence { item } => {
            push_wrapped_detail(&mut out, "id", item.id.as_str(), width);
            if let Some(agent) = item.agent_id.as_deref() {
                push_wrapped_detail(&mut out, "agent", agent, width);
            }
            if let Some(mission_id) = item.mission_id.as_deref() {
                push_wrapped_detail(&mut out, "mission", mission_id, width);
            } else {
                push_wrapped_detail(&mut out, "mission", "ad-hoc", width);
            }
            push_wrapped_detail(&mut out, "title", item.title.as_str(), width);
            push_wrapped_detail(&mut out, "detail", item.detail.as_str(), width);
            if let Some(link) = item.link.as_deref() {
                push_wrapped_detail(&mut out, "link", link, width);
            }
        }
        ArtifactRef::SwarmTask {
            mission_id,
            task_id,
        } => {
            let Some(view) = swarm.swarm_persistence(mission_id.as_str()) else {
                out.push(" Swarm persistence not available.".into());
                return out;
            };
            let Some(task) = view.tasks.iter().find(|task| task.id == *task_id) else {
                out.push(" Swarm task not found.".into());
                return out;
            };
            append_swarm_task_detail_lines(&mut out, &view, task, width);
        }
        ArtifactRef::SwarmReport { mission_id } => {
            let Some(view) = swarm.swarm_persistence(mission_id.as_str()) else {
                out.push(" Swarm persistence not available.".into());
                return out;
            };
            append_swarm_report_detail_lines(&mut out, &view, width);
        }
        ArtifactRef::SwarmVerify { mission_id } => {
            let Some(view) = swarm.swarm_persistence(mission_id.as_str()) else {
                out.push(" Swarm persistence not available.".into());
                return out;
            };
            append_swarm_verify_detail_lines(&mut out, &view, width);
        }
    }

    while out.last().is_some_and(|line| line.is_empty()) {
        out.pop();
    }
    out
}

fn diagnostics_lines(state: &AppState, width: usize) -> Vec<String> {
    let usable = width.max(32);
    let mut out = vec![" DIAG".into(), "─".repeat(usable.min(240))];
    let (errors, warns, infos) = state.agents.diag_events.iter().fold(
        (0usize, 0usize, 0usize),
        |(errors, warns, infos), event| match event.severity {
            AgentAlertSeverity::Error => (errors + 1, warns, infos),
            AgentAlertSeverity::Warn => (errors, warns + 1, infos),
            AgentAlertSeverity::Info => (errors, warns, infos + 1),
        },
    );
    let issues = state
        .agents
        .diag_events
        .iter()
        .filter(|event| {
            matches!(
                event.severity,
                AgentAlertSeverity::Error | AgentAlertSeverity::Warn
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    out.push(" Summary".into());
    push_wrapped_kv_line(&mut out, "app", diagnostics_app_label(state), usable);
    push_wrapped_kv_line(&mut out, "job", &diagnostics_job_status(state), usable);
    if let Some(accel) = diagnostics_accelerator_status(state) {
        push_wrapped_kv_line(&mut out, "accel", &accel, usable);
    }
    if let Some(path) = diagnostics_log_file_path(state) {
        push_wrapped_kv_line(&mut out, "log", path.as_str(), usable);
    }
    push_wrapped_kv_line(
        &mut out,
        "diagnostics",
        &format!("{errors} error  {warns} warn  {infos} info"),
        usable,
    );
    if let Some(issue) = issues.last() {
        push_wrapped_kv_line(
            &mut out,
            "latest_issue",
            &format_diagnostic_event(issue),
            usable,
        );
    } else {
        push_wrapped_kv_line(&mut out, "latest_issue", "none", usable);
    }
    out.push(String::new());
    out.push(" Recent issues".into());
    if issues.is_empty() {
        push_wrapped_prefixed_line(&mut out, " · ", "   ", "none", usable);
    } else {
        let recent = if issues.len() > 12 {
            &issues[issues.len().saturating_sub(12)..]
        } else {
            issues.as_slice()
        };
        for event in recent {
            push_wrapped_diagnostic_event(&mut out, event, usable);
        }
    }

    let mut logs = state.logs.iter().cloned().collect::<Vec<_>>();
    if logs.len() > 12 {
        logs = logs.split_off(logs.len() - 12);
    }
    if !logs.is_empty() {
        out.push(String::new());
        out.push(" Runtime log tail".into());
        for line in logs {
            push_wrapped_prefixed_line(
                &mut out,
                " · ",
                "   ",
                &compact_runtime_log_line(&line),
                usable,
            );
        }
    }
    out
}

fn diagnostics_app_label(state: &AppState) -> &'static str {
    match state.app_kind {
        AppKind::Gol => "gol",
        AppKind::Games => "games",
    }
}

fn diagnostics_job_status(state: &AppState) -> String {
    match state.app_kind {
        AppKind::Games => match state.games.status {
            nit_core::GamesStatus::Idle => "games/idle".into(),
            nit_core::GamesStatus::Running => "games/running".into(),
            nit_core::GamesStatus::Paused => "games/paused".into(),
            nit_core::GamesStatus::Done => "games/done".into(),
            nit_core::GamesStatus::Error => "games/error".into(),
        },
        AppKind::Gol => {
            let state = if state.visualizer.running {
                if state.visualizer.paused {
                    "gol/paused"
                } else {
                    "gol/running"
                }
            } else {
                "gol/idle"
            };
            state.into()
        }
    }
}

fn diagnostics_accelerator_status(state: &AppState) -> Option<String> {
    if !matches!(state.app_kind, AppKind::Games) {
        return None;
    }
    let runtime = &state.games.runtime;
    match runtime.backend {
        nit_games::RuntimeAcceleratorBackend::Metal => Some(format!(
            "metal active (gpu {} / cpu {})",
            runtime.metal_matches, runtime.cpu_matches
        )),
        nit_games::RuntimeAcceleratorBackend::Cpu if state.games.running => {
            Some(format!("cpu (matches {})", runtime.cpu_matches))
        }
        _ => None,
    }
}

fn format_diagnostic_event(event: &nit_core::AgentDiagnosticEvent) -> String {
    format!(
        "{} {} {} {}",
        event.at,
        event.severity.label(),
        event.source,
        event.message
    )
}

fn compact_runtime_log_line(line: &str) -> String {
    let mut parts = line.splitn(4, ' ');
    let Some(timestamp) = parts.next() else {
        return line.to_string();
    };
    let Some(level) = parts.next() else {
        return line.to_string();
    };
    let Some(target) = parts.next() else {
        return line.to_string();
    };
    let Some(message) = parts.next() else {
        return line.to_string();
    };
    let time = timestamp
        .rsplit('T')
        .next()
        .unwrap_or(timestamp)
        .trim_end_matches('Z')
        .split('.')
        .next()
        .unwrap_or(timestamp);
    let target = target.trim_end_matches(':');
    format!("{time} {level} {target} {message}")
}

fn diagnostics_log_file_path(state: &AppState) -> Option<String> {
    let marker = "Log file:";
    let mut candidate: Option<String> = None;
    for line in state.logs.iter() {
        if let Some((_, path)) = line.split_once(marker) {
            let path = path.trim();
            if !path.is_empty() {
                candidate = Some(path.to_string());
            }
        }
    }
    candidate
}

fn push_wrapped_kv_line(out: &mut Vec<String>, label: &str, value: &str, width: usize) {
    let prefix = format!(" {:<11} ", format!("{label}:"));
    push_wrapped_prefixed_line(out, &prefix, &" ".repeat(prefix.len()), value, width);
}

fn push_wrapped_diagnostic_event(
    out: &mut Vec<String>,
    event: &nit_core::AgentDiagnosticEvent,
    width: usize,
) {
    let prefix = format!(
        " {:<5} {:<7} {:<10} ",
        event.severity.label(),
        event.at,
        event.source
    );
    push_wrapped_prefixed_line(
        out,
        &prefix,
        &" ".repeat(prefix.len()),
        &event.message,
        width,
    );
}

fn push_wrapped_prefixed_line(
    out: &mut Vec<String>,
    first_prefix: &str,
    continuation_prefix: &str,
    value: &str,
    width: usize,
) {
    let available = width.saturating_sub(first_prefix.chars().count()).max(1);
    let wrapped = wrap_cell_text(value, available);
    for (idx, line) in wrapped.into_iter().enumerate() {
        let prefix = if idx == 0 {
            first_prefix
        } else {
            continuation_prefix
        };
        out.push(format!("{prefix}{line}"));
    }
}

fn scratchpad_lines(state: &AppState, width: usize) -> Vec<String> {
    let mut out = vec![
        " SCRATCHPAD".into(),
        "─".repeat(width.min(240)),
        " (legacy NOTES content)".into(),
    ];
    for line_idx in 0..state.notes_buffer().lines_len() {
        let mut line = state.notes_buffer().line_as_string(line_idx);
        if line.ends_with('\n') {
            line.pop();
        }
        out.push(fit_left(&line, width.saturating_sub(1)));
    }
    if state.notes_buffer().lines_len() == 0 {
        out.push(" <empty>".into());
    }
    out
}

fn alert_severity_label(severity: AgentAlertSeverity) -> &'static str {
    severity.label()
}

fn ops_styled_line(
    state: &AppState,
    line_idx: usize,
    line: &str,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    // Preserve the existing scroll semantics by styling the same string lines that
    // `current_lines_for_width()` produces (so mouse selection/copy still matches).
    if line_idx == 0 {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.title),
        ));
    }
    // Most tabs render a title row followed by a divider row. The roster tab uses a backend row on
    // line 1, so only apply the divider styling when the line is actually a divider.
    if line_idx == 1 && !line.is_empty() && line.chars().all(|c| c == '─') {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ));
    }

    let usable = width.max(32);
    match state.agents.dock_tab {
        AgentOpsTab::Roster => roster_styled_line(state, line_idx, line, usable, theme),
        AgentOpsTab::Missions => mission_styled_line(state, line_idx, line, usable, theme),
        AgentOpsTab::Mcp => mcp_styled_line(state, line_idx, line, usable, theme),
        AgentOpsTab::Alerts => alert_styled_line(state, line_idx, line, usable, theme),
        AgentOpsTab::Dag => dag_styled_line(line_idx, line, usable, theme),
        AgentOpsTab::Diagnostics => diagnostics_styled_line(line, theme),
        AgentOpsTab::Evidence => artifacts_styled_line(state, line_idx, line, usable, theme),
        // Patch is legacy/hidden; treat it as Diagnostics.
        AgentOpsTab::Patch => diagnostics_styled_line(line, theme),
        AgentOpsTab::Scratchpad => Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        )),
    }
}

fn artifacts_styled_line(
    _state: &AppState,
    line_idx: usize,
    line: &str,
    usable: usize,
    theme: &Theme,
) -> Line<'static> {
    if line.is_empty() {
        return Line::from("");
    }
    if line.chars().all(|ch| ch == '─') {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ));
    }
    if matches!(line, " Items") {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if is_artifacts_card_row(line) {
        let selected = line.starts_with('>');
        let is_child = is_artifacts_child_card_row(line);
        let is_root = is_artifacts_root_card_row(line);
        // Identify global cards (REPORT/VERIFY) by the kind column
        // (3 chars in: marker + glyph + space).
        let after_glyph: String = if is_root {
            line.chars().skip(3).collect()
        } else {
            String::new()
        };
        let is_report = is_root && after_glyph.starts_with("REPORT");
        let is_verify = is_root && after_glyph.starts_with("VERIFY");

        let verify_fg = if is_verify {
            if line.contains("FAIL") {
                theme.error
            } else if line.contains("PASS") {
                theme.title_focused
            } else {
                theme.warning
            }
        } else {
            theme.foreground
        };

        let style = if is_report {
            let bg = if selected {
                theme.selection_bg
            } else {
                dim_bg_towards(theme.border, theme.background, 85)
            };
            Style::default()
                .fg(theme.accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else if is_verify {
            let bg = if selected {
                theme.selection_bg
            } else {
                dim_bg_towards(theme.border, theme.background, 85)
            };
            Style::default()
                .fg(verify_fg)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default().fg(theme.foreground).bg(theme.selection_bg)
        } else if is_child {
            // Agent artifact rows: subtle background to distinguish from prompt.
            Style::default().fg(theme.foreground).bg(dim_bg_towards(
                theme.cursor_line_bg,
                theme.background,
                60,
            ))
        } else {
            // Prompt rows: dimmed cyan + bold.
            Style::default()
                .fg(Color::Rgb(100, 190, 200))
                .add_modifier(Modifier::BOLD)
        };
        return Line::from(Span::styled(line.to_string(), style));
    }

    let selected = line.starts_with('>');
    let base = if selected {
        Style::default().fg(theme.foreground).bg(theme.selection_bg)
    } else {
        Style::default().fg(theme.foreground)
    };

    let trimmed = line.trim_start();
    if trimmed.contains(':') {
        let label_style = base.fg(theme.border).add_modifier(Modifier::DIM);
        let mut spans = Vec::new();
        let leading_spaces = line.bytes().take_while(|b| *b == b' ').count();
        if leading_spaces > 0 {
            spans.push(Span::styled(" ".repeat(leading_spaces), base));
        }
        let segments = trimmed.split('|').collect::<Vec<_>>();
        for (idx, seg) in segments.iter().enumerate() {
            let (label, value) = seg
                .split_once(':')
                .map(|(l, v)| (l.trim(), v.trim()))
                .unwrap_or((seg.trim(), ""));
            spans.push(Span::styled(format!("{label}:"), label_style));
            if !value.is_empty() {
                spans.push(Span::styled(" ".to_string(), base));
                spans.push(Span::styled(
                    value.to_string(),
                    artifact_kv_value_style(label, value, base, usable, theme),
                ));
            }
            if idx + 1 < segments.len() {
                spans.push(Span::styled(" | ".to_string(), label_style));
            }
        }
        return Line::from(spans);
    }

    Line::from(Span::styled(
        line.to_string(),
        ops_line_style(line_idx, line, theme),
    ))
}

fn artifact_kv_value_style(
    label: &str,
    value: &str,
    base: Style,
    _usable: usize,
    theme: &Theme,
) -> Style {
    let label = label.trim().to_ascii_lowercase();
    let value = value.trim();

    match label.as_str() {
        "agent" => base.fg(theme.warning).add_modifier(Modifier::BOLD),
        "context" => base.fg(theme.accent).add_modifier(Modifier::BOLD),
        "thread" => {
            if value.eq_ignore_ascii_case("yes") {
                base.fg(theme.success).add_modifier(Modifier::BOLD)
            } else if value.eq_ignore_ascii_case("no") {
                base.fg(theme.border).add_modifier(Modifier::DIM)
            } else {
                base.fg(theme.hl.link).add_modifier(Modifier::UNDERLINED)
            }
        }
        "msgs" | "replies" | "patches" | "agents" | "evidence" => {
            if value.parse::<u64>().unwrap_or(0) == 0 {
                base.fg(theme.border).add_modifier(Modifier::DIM)
            } else {
                base.fg(theme.accent).add_modifier(Modifier::BOLD)
            }
        }
        "mission" | "mode" | "phase" | "status" => {
            base.fg(theme.accent).add_modifier(Modifier::BOLD)
        }
        "codex_thread" | "claude_session" | "root" | "run" | "path" | "log" | "link" => {
            base.fg(theme.hl.link).add_modifier(Modifier::UNDERLINED)
        }
        "note" => base.fg(theme.border).add_modifier(Modifier::DIM),
        _ => base,
    }
}

fn diagnostics_styled_line(line: &str, theme: &Theme) -> Line<'static> {
    if line.is_empty() {
        return Line::from("");
    }
    if matches!(line, " Summary" | " Recent issues" | " Runtime log tail") {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some((label, value)) = line
        .trim_start_matches(' ')
        .split_once(' ')
        .filter(|(label, _)| label.ends_with(':'))
    {
        let label_text = format!(" {label} ");
        let label_span = Span::styled(
            label_text,
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        );
        if label.eq_ignore_ascii_case("latest_issue:") {
            let value = value.trim_start();
            if value.eq_ignore_ascii_case("none") {
                return Line::from(vec![
                    label_span,
                    Span::styled(value.to_string(), Style::default().fg(theme.border)),
                ]);
            }
            let mut parts = value.splitn(4, ' ');
            let sev = parts.next().unwrap_or_default();
            let at = parts.next().unwrap_or_default();
            let source = parts.next().unwrap_or_default();
            let message = parts.next().unwrap_or_default();
            let sev_color = if sev.eq_ignore_ascii_case("ERROR") {
                theme.error
            } else if sev.eq_ignore_ascii_case("WARN") {
                theme.warning
            } else {
                theme.title
            };
            return Line::from(vec![
                label_span,
                Span::styled(
                    format!("{sev} "),
                    Style::default().fg(sev_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{at} "),
                    Style::default()
                        .fg(theme.border)
                        .add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    format!("{source} "),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(message.to_string(), Style::default().fg(sev_color)),
            ]);
        }
        if label.eq_ignore_ascii_case("log:") {
            return Line::from(vec![
                label_span,
                Span::styled(
                    value.to_string(),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
        }
        return Line::from(vec![
            label_span,
            Span::styled(value.to_string(), Style::default().fg(theme.foreground)),
        ]);
    }
    if let Some(rest) = line.strip_prefix(" ERROR ") {
        return diagnostics_severity_line(" ERROR ", rest, theme.error, theme);
    }
    if let Some(rest) = line.strip_prefix(" WARN  ") {
        return diagnostics_severity_line(" WARN  ", rest, theme.warning, theme);
    }
    if let Some(rest) = line.strip_prefix(" INFO  ") {
        return diagnostics_severity_line(" INFO  ", rest, theme.title, theme);
    }
    if let Some(rest) = line.strip_prefix(" · ") {
        return diagnostics_runtime_line(rest, theme);
    }
    if line.starts_with("   ") {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        ));
    }
    Line::from(Span::styled(
        line.to_string(),
        Style::default().fg(theme.foreground),
    ))
}

fn diagnostics_severity_line(
    prefix: &str,
    rest: &str,
    severity_color: Color,
    theme: &Theme,
) -> Line<'static> {
    let mut parts = rest.splitn(3, ' ');
    let at = parts.next().unwrap_or_default();
    let source = parts.next().unwrap_or_default();
    let message = parts.next().unwrap_or_default();
    Line::from(vec![
        Span::styled(prefix.to_string(), Style::default().fg(severity_color)),
        Span::styled(
            format!("{at} "),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            format!("{source} "),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(message.to_string(), Style::default().fg(severity_color)),
    ])
}

fn diagnostics_runtime_line(rest: &str, theme: &Theme) -> Line<'static> {
    let mut parts = rest.splitn(4, ' ');
    let time = parts.next().unwrap_or_default();
    let level = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let message = parts.next().unwrap_or_default();
    let level_color = if level.eq_ignore_ascii_case("ERROR") {
        theme.error
    } else if level.eq_ignore_ascii_case("WARN") {
        theme.warning
    } else {
        theme.title
    };
    Line::from(vec![
        Span::styled(
            " · ",
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            format!("{time} "),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            format!("{level} "),
            Style::default()
                .fg(level_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{target} "), Style::default().fg(theme.accent)),
        Span::styled(message.to_string(), Style::default().fg(theme.foreground)),
    ])
}

fn ops_table_bg(theme: &Theme) -> Color {
    dim_bg_towards(theme.border, theme.background, 85)
}

fn dag_table_bg(theme: &Theme) -> Color {
    ops_table_bg(theme)
}

fn dag_state_style(state: &str, base: Style, theme: &Theme) -> Style {
    let state = state.trim();
    if state.eq_ignore_ascii_case("running") {
        base.fg(theme.title_focused)
    } else if state.eq_ignore_ascii_case("queued") || state.eq_ignore_ascii_case("pending") {
        base.fg(theme.accent)
    } else if state.eq_ignore_ascii_case("failed") || state.eq_ignore_ascii_case("fail") {
        base.fg(theme.error)
    } else if state.eq_ignore_ascii_case("done")
        || state.eq_ignore_ascii_case("skipped")
        || state.eq_ignore_ascii_case("skip")
    {
        base.fg(theme.border).add_modifier(Modifier::DIM)
    } else if state.eq_ignore_ascii_case("plan")
        || state.eq_ignore_ascii_case("verify")
        || state.eq_ignore_ascii_case("synth")
    {
        base.fg(theme.accent).add_modifier(Modifier::BOLD)
    } else if state.eq_ignore_ascii_case("idle") || state.eq_ignore_ascii_case("empty") {
        base.fg(theme.border).add_modifier(Modifier::DIM)
    } else {
        base.fg(theme.foreground)
    }
}

fn dag_gate_status_style(status: &str, base: Style, theme: &Theme) -> Style {
    match status {
        "PASS" => base.fg(theme.title_focused),
        "FAIL" => base.fg(theme.error),
        "PENDING" => base.fg(theme.border).add_modifier(Modifier::DIM),
        _ => base.fg(theme.foreground),
    }
}

fn dag_styled_line(_line_idx: usize, line: &str, usable: usize, theme: &Theme) -> Line<'static> {
    let cols_total = usable.saturating_sub(1);
    let bg = dag_table_bg(theme);
    let base = Style::default().bg(bg);
    let header_style = base.fg(theme.border).add_modifier(Modifier::DIM);
    let row_style = base.fg(theme.foreground);
    let dim_row_style = base.fg(theme.border).add_modifier(Modifier::DIM);

    let trimmed = line.trim_start();
    let kv_label = trimmed.split_once(':').map(|(label, _)| label.trim());
    if kv_label.is_some_and(|label| {
        label.eq_ignore_ascii_case("status")
            || label.eq_ignore_ascii_case("mission")
            || label.eq_ignore_ascii_case("template")
            || label.eq_ignore_ascii_case("phase")
    }) {
        let label_style = base.fg(theme.border).add_modifier(Modifier::DIM);
        let mut spans = Vec::new();
        spans.push(Span::styled(" ", row_style));
        let segments = trimmed.split('|').collect::<Vec<_>>();
        for (idx, seg) in segments.iter().enumerate() {
            let (label, value) = seg
                .split_once(':')
                .map(|(l, v)| (l.trim(), v.trim()))
                .unwrap_or((seg.trim(), ""));
            spans.push(Span::styled(format!("{label}:"), label_style));
            if !value.is_empty() {
                spans.push(Span::styled(" ", row_style));
                let value_style = if label.eq_ignore_ascii_case("status") {
                    dag_state_style(value, row_style, theme)
                } else if label.eq_ignore_ascii_case("phase") {
                    row_style.fg(theme.accent).add_modifier(Modifier::BOLD)
                } else {
                    row_style
                };
                spans.push(Span::styled(value.to_string(), value_style));
            }
            if idx + 1 < segments.len() {
                spans.push(Span::styled(" | ", label_style));
            }
        }
        return Line::from(spans);
    }

    if kv_label.is_some_and(|label| {
        label.eq_ignore_ascii_case("done")
            || label.eq_ignore_ascii_case("fail")
            || label.eq_ignore_ascii_case("run")
            || label.eq_ignore_ascii_case("queue")
            || label.eq_ignore_ascii_case("pending")
            || label.eq_ignore_ascii_case("skip")
    }) {
        let label_style = base.fg(theme.border).add_modifier(Modifier::DIM);
        let mut spans = Vec::new();
        spans.push(Span::styled(" ", row_style));
        let segments = trimmed.split('|').collect::<Vec<_>>();
        for (idx, seg) in segments.iter().enumerate() {
            let (label, value) = seg
                .split_once(':')
                .map(|(l, v)| (l.trim(), v.trim()))
                .unwrap_or((seg.trim(), ""));
            let value_style = if label.eq_ignore_ascii_case("fail") {
                row_style.fg(theme.error)
            } else if label.eq_ignore_ascii_case("run") {
                row_style.fg(theme.title_focused)
            } else if label.eq_ignore_ascii_case("queue") || label.eq_ignore_ascii_case("pending") {
                row_style.fg(theme.accent)
            } else if label.eq_ignore_ascii_case("done") {
                row_style.fg(theme.title)
            } else if label.eq_ignore_ascii_case("skip") {
                row_style.fg(theme.border).add_modifier(Modifier::DIM)
            } else {
                row_style
            };
            spans.push(Span::styled(format!("{label}:"), label_style));
            if !value.is_empty() {
                spans.push(Span::styled(" ", row_style));
                spans.push(Span::styled(value.to_string(), value_style));
            }
            if idx + 1 < segments.len() {
                spans.push(Span::styled(" | ", label_style));
            }
        }
        return Line::from(spans);
    }

    if trimmed.starts_with("Gate:") {
        let label_style = base.fg(theme.border).add_modifier(Modifier::DIM);
        let value = trimmed.strip_prefix("Gate:").unwrap_or(trimmed).trim();
        let spans = vec![
            Span::styled(" ", row_style),
            Span::styled("Gate:", label_style),
            Span::styled(" ", row_style),
            Span::styled(value.to_string(), row_style.fg(theme.title_focused)),
        ];
        return Line::from(spans);
    }

    if trimmed.starts_with("Swarm DAG") || trimmed.starts_with("Gate bundle:") {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        ));
    }
    if line.starts_with('─') {
        return Line::from(Span::styled(line.to_string(), dim_row_style));
    }

    if trimmed.starts_with("ID") {
        let widths = dag_task_widths(cols_total);
        let Some((marker, cols)) = split_marker_and_columns(line, &widths) else {
            return Line::from(Span::styled(line.to_string(), header_style));
        };
        let mut spans = Vec::with_capacity(cols.len().saturating_mul(2) + 1);
        spans.push(Span::styled(marker, header_style));
        for (idx, col) in cols.into_iter().enumerate() {
            spans.push(Span::styled(col, header_style));
            if idx + 1 < widths.len() {
                spans.push(Span::styled(" ", header_style));
            }
        }
        return Line::from(spans);
    }

    if trimmed.starts_with("GATE") {
        let widths = dag_gate_widths(cols_total);
        let Some((marker, cols)) = split_marker_and_columns(line, &widths) else {
            return Line::from(Span::styled(line.to_string(), header_style));
        };
        let mut spans = Vec::with_capacity(cols.len().saturating_mul(2) + 1);
        spans.push(Span::styled(marker, header_style));
        for (idx, col) in cols.into_iter().enumerate() {
            spans.push(Span::styled(col, header_style));
            if idx + 1 < widths.len() {
                spans.push(Span::styled(" ", header_style));
            }
        }
        return Line::from(spans);
    }

    // Gate rows: GATE | STATUS | COMMAND
    let gate_widths = dag_gate_widths(cols_total);
    if let Some((marker, cols)) = split_marker_and_columns(line, &gate_widths) {
        let status = cols.get(1).map(|s| s.trim()).unwrap_or_default();
        if matches!(status, "PENDING" | "PASS" | "FAIL") {
            let gate_style = row_style.fg(theme.title_focused);
            let status_style = dag_gate_status_style(status, row_style, theme);
            let cmd_style = if cols.first().map(|s| s.trim().is_empty()).unwrap_or(false) {
                row_style.add_modifier(Modifier::DIM)
            } else {
                row_style
            };
            let space_style = row_style;
            let mut spans = Vec::with_capacity(8);
            spans.push(Span::styled(marker, row_style));
            spans.push(Span::styled(
                cols.first().cloned().unwrap_or_default(),
                gate_style,
            ));
            spans.push(Span::styled(" ", space_style));
            spans.push(Span::styled(
                cols.get(1).cloned().unwrap_or_default(),
                status_style,
            ));
            spans.push(Span::styled(" ", space_style));
            spans.push(Span::styled(
                cols.get(2).cloned().unwrap_or_default(),
                cmd_style,
            ));
            return Line::from(spans);
        }
    }

    // Task cards: ID | STATE | TITLE (variable-height; details lines have empty ID/STATE).
    if trimmed.starts_with("No ")
        || trimmed.starts_with("Mission ")
        || trimmed.starts_with("Swarm runtime")
        || trimmed.starts_with("No DAG data")
        || trimmed.starts_with("Planning:")
        || trimmed.starts_with("No tasks.")
    {
        return Line::from(Span::styled(line.to_string(), dim_row_style));
    }

    let task_widths = dag_task_widths(cols_total);
    let Some((marker, cols)) = split_marker_and_columns(line, &task_widths) else {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        ));
    };
    let id_cell = cols.first().cloned().unwrap_or_default();
    let state_cell = cols.get(1).cloned().unwrap_or_default();
    let title_cell = cols.get(2).cloned().unwrap_or_default();

    let is_details = id_cell.trim().is_empty()
        && state_cell.trim().is_empty()
        && title_cell.trim_start().starts_with(arrow_glyph());
    let marker_style = row_style.fg(theme.border).add_modifier(Modifier::DIM);
    let id_style = if is_details {
        dim_row_style
    } else {
        row_style.fg(theme.title_focused)
    };
    let state_style = if is_details {
        dim_row_style
    } else {
        dag_state_style(state_cell.trim(), row_style, theme)
    };
    let title_style = if is_details {
        row_style.add_modifier(Modifier::DIM)
    } else {
        row_style
    };
    let space_style = row_style;

    let mut spans = Vec::with_capacity(10);
    spans.push(Span::styled(marker, marker_style));
    spans.push(Span::styled(id_cell, id_style));
    spans.push(Span::styled(" ", space_style));
    spans.push(Span::styled(state_cell, state_style));
    spans.push(Span::styled(" ", space_style));
    spans.push(Span::styled(title_cell, title_style));
    Line::from(spans)
}

fn selected_row_style(style: Style, selected: bool, theme: &Theme) -> Style {
    if selected {
        style.bg(theme.selection_bg)
    } else {
        style
    }
}

fn striped_row_style(style: Style, selected: bool, striped: bool, theme: &Theme) -> Style {
    if selected {
        style.bg(theme.selection_bg)
    } else if striped {
        // Mission zebra stripes should read as "dim background", clearly distinct from the selected
        // row highlight. Derive the stripe bg from the theme instead of hardcoding colors.
        style.bg(dim_bg_towards(theme.cursor_line_bg, theme.background, 60))
    } else {
        style
    }
}

fn dim_bg_towards(color: Color, background: Color, background_pct: u8) -> Color {
    let pct = background_pct.min(100) as u16;
    match (color, background) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r0, g0, b0)) => {
            let inv = 100u16.saturating_sub(pct);
            let mix = |top: u8, base: u8| -> u8 {
                let top = top as u16;
                let base = base as u16;
                ((top.saturating_mul(inv) + base.saturating_mul(pct) + 50) / 100) as u8
            };
            Color::Rgb(mix(r1, r0), mix(g1, g0), mix(b1, b0))
        }
        _ => color,
    }
}

fn agent_status_style(status: AgentStatus, theme: &Theme) -> Style {
    match status {
        AgentStatus::Running => Style::default().fg(theme.title_focused),
        AgentStatus::Waiting => Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        AgentStatus::Idle => Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        AgentStatus::Error => Style::default().fg(theme.error),
    }
}

fn heartbeat_age_style(age_secs: u64, theme: &Theme) -> Style {
    if age_secs <= 2 {
        Style::default().fg(theme.title_focused)
    } else if age_secs <= 5 {
        Style::default().fg(theme.warning)
    } else {
        Style::default().fg(theme.error)
    }
}

fn queue_len_style(queue_len: usize, theme: &Theme) -> Style {
    if queue_len > 0 {
        Style::default().fg(theme.accent)
    } else {
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM)
    }
}

fn alert_severity_style(severity: AgentAlertSeverity, theme: &Theme) -> Style {
    match severity {
        AgentAlertSeverity::Info => Style::default().fg(theme.title_focused),
        AgentAlertSeverity::Warn => Style::default().fg(theme.warning),
        AgentAlertSeverity::Error => Style::default().fg(theme.error),
    }
}

fn mcp_state_style(state: McpConnectionState, theme: &Theme) -> Style {
    match state {
        McpConnectionState::Connected => Style::default().fg(theme.title_focused),
        McpConnectionState::Connecting => Style::default().fg(theme.hl.operator),
        McpConnectionState::Disconnected => Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        McpConnectionState::Error => Style::default().fg(theme.error),
    }
}

fn take_chars(text: &str, start: usize, len: usize) -> String {
    text.chars().skip(start).take(len).collect()
}

fn split_marker_and_columns(line: &str, widths: &[usize]) -> Option<(String, Vec<String>)> {
    if widths.is_empty() {
        return None;
    }
    let mut pos = 0usize;
    let marker = take_chars(line, pos, 1);
    pos = pos.saturating_add(1);
    let mut cols = Vec::with_capacity(widths.len());
    for (idx, w) in widths.iter().enumerate() {
        cols.push(take_chars(line, pos, *w));
        pos = pos.saturating_add(*w);
        if idx + 1 < widths.len() {
            // Single spacer between fixed-width columns.
            pos = pos.saturating_add(1);
        }
    }
    Some((marker, cols))
}

fn roster_backend_styled_line(
    name: &str,
    primary: &str,
    secondary: &str,
    accent: Color,
    available: bool,
    active: bool,
    theme: &Theme,
) -> Line<'static> {
    let name_style = if available {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM)
    };
    let primary_style = if available {
        Style::default().fg(theme.foreground)
    } else {
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM)
    };
    let secondary_style = if active {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM)
    };

    Line::from(vec![
        Span::styled("  ", Style::default().fg(theme.foreground)),
        Span::styled(format!("{name:<ROSTER_BACKEND_NAME_W$}"), name_style),
        Span::styled("  ", Style::default().fg(theme.foreground)),
        Span::styled(primary.to_string(), primary_style),
        Span::styled("  ", Style::default().fg(theme.foreground)),
        Span::styled(secondary.to_string(), secondary_style),
    ])
}

fn roster_styled_line(
    state: &AppState,
    line_idx: usize,
    line: &str,
    usable: usize,
    theme: &Theme,
) -> Line<'static> {
    let table_bg = ops_table_bg(theme);
    let offsets = roster_header_offsets(state);
    if line_idx == 0 {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.title),
        ));
    }

    if line_idx >= 1 && line_idx < offsets.blank_after_backends {
        let rows = roster_backend_inventory_rows(state);
        let Some(row) = rows.get(line_idx.saturating_sub(1)) else {
            return Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(theme.foreground),
            ));
        };

        match row.backend {
            BackendInventoryBackend::Codex => {
                return roster_backend_styled_line(
                    "Codex",
                    if row.available {
                        "available"
                    } else {
                        "not found"
                    },
                    if row.active { "active" } else { "idle" },
                    roster_inventory_backend_accent(row.backend, theme),
                    row.available,
                    row.active,
                    theme,
                );
            }
            BackendInventoryBackend::Claude => {
                return roster_backend_styled_line(
                    "Claude",
                    if row.available {
                        "available"
                    } else {
                        "not found"
                    },
                    if row.active { "active" } else { "idle" },
                    roster_inventory_backend_accent(row.backend, theme),
                    row.available,
                    row.active,
                    theme,
                );
            }
            BackendInventoryBackend::Gemini => {
                return roster_backend_styled_line(
                    "Gemini",
                    if row.available {
                        "available"
                    } else {
                        "not found"
                    },
                    if row.active { "active" } else { "idle" },
                    roster_inventory_backend_accent(row.backend, theme),
                    row.available,
                    row.active,
                    theme,
                );
            }
            BackendInventoryBackend::Local => {
                return roster_backend_styled_line(
                    "Local",
                    "built-in",
                    if row.active { "active" } else { "idle" },
                    roster_inventory_backend_accent(row.backend, theme),
                    true,
                    row.active,
                    theme,
                );
            }
        }
    }

    if line_idx == offsets.blank_after_backends {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        ));
    }
    if line_idx == offsets.template_line {
        let label_style = Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM);
        let selected_style = Style::default()
            .fg(theme.background)
            .bg(theme.border_focused)
            .add_modifier(Modifier::BOLD);
        let unselected_style = Style::default().fg(theme.foreground).bg(dim_bg_towards(
            theme.cursor_line_bg,
            theme.background,
            45,
        ));

        let mut spans: Vec<Span<'static>> = Vec::with_capacity(8);
        spans.push(Span::styled(" Template: ", label_style));
        for (idx, tmpl) in ["lab", "parallel", "bulk"].iter().enumerate() {
            let selected = state
                .agents
                .swarm_default_template
                .eq_ignore_ascii_case(tmpl);
            let style = if selected {
                selected_style
            } else {
                unselected_style
            };
            spans.push(Span::styled(format!(" {tmpl} "), style));
            if idx + 1 < 3 {
                spans.push(Span::styled(" ", label_style));
            }
        }
        return Line::from(spans);
    }
    if line_idx == offsets.mission_line {
        let label_style = Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM);
        let selected_style = Style::default()
            .fg(theme.background)
            .bg(theme.border_focused)
            .add_modifier(Modifier::BOLD);
        let unselected_style = Style::default().fg(theme.foreground).bg(dim_bg_towards(
            theme.cursor_line_bg,
            theme.background,
            45,
        ));

        let mut spans: Vec<Span<'static>> = Vec::with_capacity(10);
        spans.push(Span::styled(" Mission: ", label_style));
        for (idx, (display, value)) in [
            ("auto", "auto"),
            ("general", "general"),
            ("research", "research"),
            ("computational", "computational-research"),
        ]
        .iter()
        .enumerate()
        {
            let selected = state
                .agents
                .swarm_default_mission
                .eq_ignore_ascii_case(value);
            let style = if selected {
                selected_style
            } else {
                unselected_style
            };
            spans.push(Span::styled(format!(" {display} "), style));
            if idx + 1 < 4 {
                spans.push(Span::styled(" ", label_style));
            }
        }
        return Line::from(spans);
    }
    if line_idx == offsets.blank_after_mission {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        ));
    }
    if line_idx == offsets.table_header {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.border).bg(table_bg),
        ));
    }
    if line_idx == offsets.table_separator {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM)
                .bg(table_bg),
        ));
    }
    if !line.is_empty() && line.chars().all(|c| c == '─') {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM)
                .bg(table_bg),
        ));
    }
    let body_line = line_idx.saturating_sub(roster_body_offset(state));
    let Some(meta) = roster_body_meta(state, body_line) else {
        if state.agents.agents.is_empty() {
            return Line::from(Span::styled(
                line.to_string(),
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM),
            ));
        }
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        ));
    };

    let widths = roster_column_widths(usable);
    let Some((marker, cols)) = split_marker_and_columns(line, &widths) else {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        ));
    };
    let selected_row = roster_selected_row(state);
    match meta.node {
        RosterBodyNode::Backend { backend } => {
            let selected = selected_row == Some(RosterSelectableRow::Backend { backend });
            let marker_style = selected_row_style(
                if selected {
                    Style::default().fg(theme.accent).bg(table_bg)
                } else {
                    Style::default()
                        .fg(theme.border)
                        .add_modifier(Modifier::DIM)
                        .bg(table_bg)
                },
                selected,
                theme,
            );
            let backend_style = selected_row_style(
                Style::default()
                    .fg(roster_lane_backend_accent(backend, theme))
                    .add_modifier(Modifier::BOLD)
                    .bg(table_bg),
                selected,
                theme,
            );
            let cell_style = selected_row_style(
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM)
                    .bg(table_bg),
                selected,
                theme,
            );
            let space_style = selected_row_style(Style::default().bg(table_bg), selected, theme);

            let mut spans = Vec::with_capacity(14);
            spans.push(Span::styled(marker, marker_style));
            spans.push(Span::styled(
                cols.first().cloned().unwrap_or_default(),
                backend_style,
            ));
            spans.push(Span::styled(" ", space_style));
            spans.push(Span::styled(
                cols.get(1).cloned().unwrap_or_default(),
                cell_style,
            ));
            spans.push(Span::styled(" ", space_style));
            spans.push(Span::styled(
                cols.get(2).cloned().unwrap_or_default(),
                cell_style,
            ));
            spans.push(Span::styled(" ", space_style));
            spans.push(Span::styled(
                cols.get(3).cloned().unwrap_or_default(),
                cell_style,
            ));
            spans.push(Span::styled(" ", space_style));
            spans.push(Span::styled(
                cols.get(4).cloned().unwrap_or_default(),
                cell_style,
            ));
            Line::from(spans)
        }
        RosterBodyNode::Agent => {
            let Some(agent_idx) = meta.agent_idx else {
                return Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(theme.foreground).bg(table_bg),
                ));
            };
            let Some(agent) = state.agents.agents.get(agent_idx) else {
                return Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(theme.foreground).bg(table_bg),
                ));
            };

            let selected = selected_row == Some(RosterSelectableRow::Agent { agent_idx })
                && state.agents.roster_tree_selected.is_none();
            let is_clone = is_swarm_clone_agent_id(agent.id.as_str())
                || is_chat_clone_agent_id(agent.id.as_str());

            let marker_style = if selected {
                selected_row_style(Style::default().fg(theme.accent).bg(table_bg), true, theme)
            } else {
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM)
                    .bg(table_bg)
            };

            let role_style = selected_row_style(
                if is_clone {
                    Style::default()
                        .fg(theme.border)
                        .add_modifier(Modifier::DIM)
                        .bg(table_bg)
                } else {
                    Style::default().fg(theme.foreground).bg(table_bg)
                },
                selected,
                theme,
            );
            let clone_dim = Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM)
                .bg(table_bg);
            let clone_is_active =
                is_clone && matches!(agent.status, nit_core::AgentStatus::Running);
            let status_style = selected_row_style(
                if is_clone && !clone_is_active {
                    clone_dim
                } else {
                    agent_status_style(agent.status, theme).bg(table_bg)
                },
                selected,
                theme,
            );
            let hb_style = selected_row_style(
                if is_clone {
                    clone_dim
                } else {
                    heartbeat_age_style(agent.heartbeat_age_secs, theme).bg(table_bg)
                },
                selected,
                theme,
            );
            let q_style = selected_row_style(
                if is_clone {
                    clone_dim
                } else {
                    queue_len_style(agent.queue_len, theme).bg(table_bg)
                },
                selected,
                theme,
            );
            let mission_style = selected_row_style(
                if is_clone {
                    clone_dim
                } else if agent.current_mission.is_some() {
                    Style::default().fg(theme.title).bg(table_bg)
                } else {
                    Style::default()
                        .fg(theme.border)
                        .add_modifier(Modifier::DIM)
                        .bg(table_bg)
                },
                selected,
                theme,
            );
            let space_style = selected_row_style(Style::default().bg(table_bg), selected, theme);

            let mut spans = Vec::with_capacity(14);
            spans.push(Span::styled(marker, marker_style));
            let col0 = cols.first().cloned().unwrap_or_default();
            if agent.supports_swarm_priority()
                && (col0.starts_with("[x] ") || col0.starts_with("[ ] "))
            {
                let checked = state.agents.swarm_priority_agent_ids.contains(&agent.id);
                let prefix = take_chars(&col0, 0, 4);
                let rest = take_chars(&col0, 4, col0.chars().count().saturating_sub(4));
                let base_prefix_style = if checked {
                    Style::default().fg(theme.warning).bg(table_bg)
                } else {
                    Style::default()
                        .fg(theme.border)
                        .add_modifier(Modifier::DIM)
                        .bg(table_bg)
                };
                let prefix_style = selected_row_style(base_prefix_style, selected, theme);
                spans.push(Span::styled(prefix, prefix_style));
                spans.push(Span::styled(rest, role_style));
            } else {
                spans.push(Span::styled(col0, role_style));
            }
            spans.push(Span::styled(" ", space_style));
            spans.push(Span::styled(
                cols.get(1).cloned().unwrap_or_default(),
                status_style,
            ));
            spans.push(Span::styled(" ", space_style));
            spans.push(Span::styled(
                cols.get(2).cloned().unwrap_or_default(),
                hb_style,
            ));
            spans.push(Span::styled(" ", space_style));
            spans.push(Span::styled(
                cols.get(3).cloned().unwrap_or_default(),
                q_style,
            ));
            spans.push(Span::styled(" ", space_style));
            spans.push(Span::styled(
                cols.get(4).cloned().unwrap_or_default(),
                mission_style,
            ));
            Line::from(spans)
        }
        RosterBodyNode::Branch { branch } => {
            let Some(agent_idx) = meta.agent_idx else {
                return Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(theme.foreground).bg(table_bg),
                ));
            };
            let active = agent_idx == state.agents.roster_selected
                && state
                    .agents
                    .roster_tree_selected
                    .is_some_and(|sel| sel.branch == branch);

            let marker_style = Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM)
                .bg(table_bg);
            let base_role_style = if active {
                Style::default().fg(theme.foreground).bg(table_bg)
            } else {
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM)
                    .bg(table_bg)
            };
            let role_style = selected_row_style(base_role_style, false, theme);
            let cell_style = selected_row_style(
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM)
                    .bg(table_bg),
                false,
                theme,
            );

            let mut spans = Vec::with_capacity(14);
            spans.push(Span::styled(marker, marker_style));
            spans.push(Span::styled(
                cols.first().cloned().unwrap_or_default(),
                role_style,
            ));
            spans.push(Span::styled(" ", cell_style));
            spans.push(Span::styled(
                cols.get(1).cloned().unwrap_or_default(),
                cell_style,
            ));
            spans.push(Span::styled(" ", cell_style));
            spans.push(Span::styled(
                cols.get(2).cloned().unwrap_or_default(),
                cell_style,
            ));
            spans.push(Span::styled(" ", cell_style));
            spans.push(Span::styled(
                cols.get(3).cloned().unwrap_or_default(),
                cell_style,
            ));
            spans.push(Span::styled(" ", cell_style));
            spans.push(Span::styled(
                cols.get(4).cloned().unwrap_or_default(),
                cell_style,
            ));
            Line::from(spans)
        }
        RosterBodyNode::Leaf { branch, leaf_idx } => {
            let Some(agent_idx) = meta.agent_idx else {
                return Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(theme.foreground).bg(table_bg),
                ));
            };
            let Some(agent) = state.agents.agents.get(agent_idx) else {
                return Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(theme.foreground).bg(table_bg),
                ));
            };

            let selected = agent_idx == state.agents.roster_selected
                && state.agents.roster_tree_selected
                    == Some(RosterTreeSelection { branch, leaf_idx });

            let marker_style = if selected {
                selected_row_style(Style::default().fg(theme.accent).bg(table_bg), true, theme)
            } else {
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM)
                    .bg(table_bg)
            };

            let is_chosen = match branch {
                RosterTreeBranch::Size => {
                    let chosen = state
                        .agents
                        .codex_selected_reasoning_effort
                        .get(&agent.id)
                        .or_else(|| state.agents.codex_default_reasoning_effort.get(&agent.id))
                        .or_else(|| state.agents.claude_selected_effort.get(&agent.id))
                        .or_else(|| state.agents.claude_default_effort.get(&agent.id))
                        .map(|s| s.as_str());
                    let effort = state
                        .agents
                        .codex_supported_reasoning_efforts
                        .get(&agent.id)
                        .or_else(|| state.agents.claude_supported_efforts.get(&agent.id))
                        .and_then(|v| v.get(leaf_idx))
                        .map(|s| s.as_str());
                    effort.is_some_and(|effort| chosen == Some(effort))
                }
                RosterTreeBranch::Role => {
                    let chosen = state
                        .agents
                        .swarm_role_by_agent_id
                        .get(&agent.id)
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("all");
                    let chosen = normalize_roster_role_hint(chosen);
                    ROSTER_ROLE_OPTIONS
                        .get(leaf_idx)
                        .is_some_and(|role| chosen == normalize_roster_role_hint(role))
                }
            };

            let base_role_style = if is_chosen {
                Style::default().fg(theme.title_focused).bg(table_bg)
            } else {
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM)
                    .bg(table_bg)
            };
            let role_style = selected_row_style(base_role_style, selected, theme);
            let cell_style = selected_row_style(Style::default().bg(table_bg), selected, theme);

            let mut spans = Vec::with_capacity(14);
            spans.push(Span::styled(marker, marker_style));
            spans.push(Span::styled(
                cols.first().cloned().unwrap_or_default(),
                role_style,
            ));
            spans.push(Span::styled(" ", cell_style));
            spans.push(Span::styled(
                cols.get(1).cloned().unwrap_or_default(),
                cell_style,
            ));
            spans.push(Span::styled(" ", cell_style));
            spans.push(Span::styled(
                cols.get(2).cloned().unwrap_or_default(),
                cell_style,
            ));
            spans.push(Span::styled(" ", cell_style));
            spans.push(Span::styled(
                cols.get(3).cloned().unwrap_or_default(),
                cell_style,
            ));
            spans.push(Span::styled(" ", cell_style));
            spans.push(Span::styled(
                cols.get(4).cloned().unwrap_or_default(),
                cell_style,
            ));
            Line::from(spans)
        }
    }
}

fn mission_phase_style(phase: nit_core::MissionPhase, theme: &Theme) -> Style {
    match phase {
        nit_core::MissionPhase::Plan => Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
        nit_core::MissionPhase::Execute => Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
        nit_core::MissionPhase::Verify => Style::default().fg(theme.title),
        nit_core::MissionPhase::Report => Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
    }
}

fn mission_status_style(status: &str, theme: &Theme) -> Style {
    let upper = status.to_ascii_uppercase();
    if upper.contains("ERROR") || upper.contains("FAILED") {
        Style::default().fg(theme.error)
    } else if upper.contains("WARN") {
        Style::default().fg(theme.warning)
    } else if upper.contains("RUNNING")
        || upper.contains("ACTIVE")
        || upper.contains("APPLIED")
        || upper.contains("DONE")
        || upper.contains("COMPLETE")
    {
        Style::default().fg(theme.title_focused)
    } else {
        Style::default().fg(theme.foreground)
    }
}

fn mission_styled_line(
    state: &AppState,
    line_idx: usize,
    line: &str,
    usable: usize,
    theme: &Theme,
) -> Line<'static> {
    if state.agents.missions.is_empty() {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ));
    }
    let body_idx = line_idx.saturating_sub(2);
    let Some(meta) = mission_body_meta(state, body_idx) else {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        ));
    };
    let Some(mission) = state.agents.missions.get(meta.mission_idx) else {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        ));
    };
    let cols_total = usable.saturating_sub(1);
    // Must match `mission_lines()`.
    let widths = allocate_columns(cols_total, &[6, 6, 3, 6, 8], &[12, 8, 5, 12, 18], 4);
    let Some((marker, cols)) = split_marker_and_columns(line, &widths) else {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        ));
    };
    let selected = meta.mission_idx == state.agents.mission_selected;
    let striped = !selected && meta.mission_idx % 2 == 1;

    let marker_style = if selected && meta.agent_row == Some(0) {
        striped_row_style(Style::default().fg(theme.accent), selected, striped, theme)
    } else {
        striped_row_style(
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
            selected,
            striped,
            theme,
        )
    };
    let muted_style = striped_row_style(
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        selected,
        striped,
        theme,
    );
    let is_primary_line = meta.agent_row == Some(0);
    let id_style = if is_primary_line {
        striped_row_style(
            Style::default().fg(theme.foreground),
            selected,
            striped,
            theme,
        )
    } else {
        muted_style
    };
    let phase_style = if is_primary_line {
        striped_row_style(
            mission_phase_style(mission.phase, theme),
            selected,
            striped,
            theme,
        )
    } else {
        muted_style
    };
    let swarm_style = if is_primary_line {
        striped_row_style(
            if mission.swarm {
                Style::default().fg(theme.accent)
            } else {
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM)
            },
            selected,
            striped,
            theme,
        )
    } else {
        muted_style
    };
    let status_style = if is_primary_line {
        striped_row_style(
            mission_status_style(&mission.status, theme),
            selected,
            striped,
            theme,
        )
    } else {
        muted_style
    };
    let space_style = striped_row_style(Style::default(), selected, striped, theme);

    let agent_edges = striped_row_style(
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        selected,
        striped,
        theme,
    );
    let agent_assigned = striped_row_style(
        Style::default().fg(theme.title_focused),
        selected,
        striped,
        theme,
    );
    let agent_missing = striped_row_style(
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        selected,
        striped,
        theme,
    );

    let mut spans = Vec::with_capacity(20);
    spans.push(Span::styled(marker, marker_style));
    spans.push(Span::styled(
        cols.first().cloned().unwrap_or_default(),
        id_style,
    ));
    spans.push(Span::styled(" ", space_style));
    spans.push(Span::styled(
        cols.get(1).cloned().unwrap_or_default(),
        phase_style,
    ));
    spans.push(Span::styled(" ", space_style));
    spans.push(Span::styled(
        cols.get(2).cloned().unwrap_or_default(),
        swarm_style,
    ));
    spans.push(Span::styled(" ", space_style));
    spans.push(Span::styled(
        cols.get(3).cloned().unwrap_or_default(),
        status_style,
    ));
    spans.push(Span::styled(" ", space_style));

    let agents_col = cols.get(4).cloned().unwrap_or_default();
    match meta.agent_row {
        None => spans.push(Span::styled(agents_col, agent_edges)),
        Some(agent_row) => {
            let assigned = mission.assigned_agents.get(agent_row).is_some();
            let inner = agent_pane_inner_width(widths[4]);
            spans.push(Span::styled(take_chars(&agents_col, 0, 1), agent_edges));
            spans.push(Span::styled(
                take_chars(&agents_col, 1, inner),
                if assigned {
                    agent_assigned
                } else {
                    agent_missing
                },
            ));
            spans.push(Span::styled(
                take_chars(&agents_col, widths[4].saturating_sub(1), 1),
                agent_edges,
            ));
        }
    }
    Line::from(spans)
}

fn mcp_styled_line(
    state: &AppState,
    line_idx: usize,
    line: &str,
    usable: usize,
    theme: &Theme,
) -> Line<'static> {
    let _ = line_idx; // line indices aren't stable (LAST ERR is conditional); parse by content.
    let mcp = &state.agents.mcp;
    let key_w = 11usize.min(usable.saturating_sub(4));
    let value_w = usable.saturating_sub(key_w + 2).max(1);
    if line.is_empty() {
        return Line::from(Span::styled(String::new(), Style::default()));
    }
    if line.starts_with(" [") {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ));
    }
    if line.starts_with(' ') {
        let key = take_chars(line, 1, key_w);
        let value = take_chars(line, 2 + key_w, value_w);
        if key.trim().is_empty() && value.trim().is_empty() {
            return Line::from(Span::styled(String::new(), Style::default()));
        }
        let key_style = Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM);
        let value_style = if key.trim() == "STATE" {
            mcp_state_style(mcp.state, theme)
        } else if key.trim() == "LATENCY" {
            Style::default().fg(theme.accent)
        } else if key.trim() == "LAST ERR" {
            Style::default().fg(theme.error)
        } else {
            Style::default().fg(theme.foreground)
        };
        return Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(key, key_style),
            Span::styled(" ", Style::default()),
            Span::styled(value, value_style),
        ]);
    }
    Line::from(Span::styled(
        line.to_string(),
        Style::default().fg(theme.foreground),
    ))
}

fn alert_styled_line(
    state: &AppState,
    line_idx: usize,
    line: &str,
    usable: usize,
    theme: &Theme,
) -> Line<'static> {
    if state.agents.alerts.is_empty() {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ));
    }
    let body_line = line_idx.saturating_sub(2);
    let Some(meta) = alert_body_meta(state, usable, body_line) else {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        ));
    };
    let Some(alert) = state.agents.alerts.get(meta.alert_idx) else {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        ));
    };
    let cols_total = usable.saturating_sub(1);
    let widths = allocate_columns(cols_total, &[4, 4, 5, 12], &[5, 8, 10, 26], 3);
    let Some((marker, cols)) = split_marker_and_columns(line, &widths) else {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        ));
    };
    let selected = meta.alert_idx == state.agents.alert_selected;

    let marker_style = if selected {
        selected_row_style(Style::default().fg(theme.accent), true, theme)
    } else {
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM)
    };
    let sev_style = if meta.wrap_row == 0 {
        selected_row_style(alert_severity_style(alert.severity, theme), selected, theme)
    } else {
        selected_row_style(
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
            selected,
            theme,
        )
    };
    let time_style = selected_row_style(
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        selected,
        theme,
    );
    let src_style = if meta.wrap_row == 0 {
        selected_row_style(Style::default().fg(theme.title), selected, theme)
    } else {
        selected_row_style(
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
            selected,
            theme,
        )
    };
    let msg_style = selected_row_style(Style::default().fg(theme.foreground), selected, theme);
    let space_style = selected_row_style(Style::default(), selected, theme);

    let mut spans = Vec::with_capacity(12);
    spans.push(Span::styled(marker, marker_style));
    spans.push(Span::styled(
        cols.first().cloned().unwrap_or_default(),
        sev_style,
    ));
    spans.push(Span::styled(" ", space_style));
    spans.push(Span::styled(
        cols.get(1).cloned().unwrap_or_default(),
        time_style,
    ));
    spans.push(Span::styled(" ", space_style));
    spans.push(Span::styled(
        cols.get(2).cloned().unwrap_or_default(),
        src_style,
    ));
    spans.push(Span::styled(" ", space_style));
    spans.push(Span::styled(
        cols.get(3).cloned().unwrap_or_default(),
        msg_style,
    ));
    Line::from(spans)
}

fn ops_line_style(line_idx: usize, line: &str, theme: &Theme) -> Style {
    if line_idx == 0 {
        return Style::default().fg(theme.title);
    }
    if line_idx == 1 {
        return Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM);
    }
    if line.chars().all(|ch| ch == '─') {
        return Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM);
    }
    let selected = line.starts_with('>');
    let mut style = Style::default().fg(theme.foreground);
    if line.contains("ERROR") || line.contains("REJECTED") {
        style = style.fg(theme.error);
    } else if line.contains("WARN") {
        style = style.fg(theme.warning);
    } else if line.contains("RUNNING")
        || line.contains("CONNECTED")
        || line.contains("APPLIED")
        || line.contains("REVIEWED")
        || (line.starts_with('+') && !line.starts_with("+++"))
    {
        style = style.fg(theme.title_focused);
    } else if line.starts_with('-') && !line.starts_with("---") {
        style = style.fg(theme.error);
    } else if line.contains("NEW") || line.contains("PLAN") {
        style = style.fg(theme.accent);
    } else if line.contains("IDLE") || line.contains("DISCONNECTED") {
        style = style.fg(theme.border).add_modifier(Modifier::DIM);
    }
    if selected {
        style = style.bg(theme.selection_bg);
    }
    style
}

fn allocate_columns(
    total: usize,
    mins: &[usize],
    prefs: &[usize],
    separators: usize,
) -> Vec<usize> {
    let mut widths = prefs.to_vec();
    let min_sum = mins.iter().sum::<usize>() + separators;
    if total <= min_sum {
        let mut compact = mins.to_vec();
        if total > separators {
            let mut overflow = min_sum.saturating_sub(total);
            for idx in (0..compact.len()).rev() {
                if overflow == 0 {
                    break;
                }
                let reducible = compact[idx].saturating_sub(1);
                let take = reducible.min(overflow);
                compact[idx] = compact[idx].saturating_sub(take);
                overflow -= take;
            }
        }
        return compact;
    }
    let mut used = widths.iter().sum::<usize>() + separators;
    if used > total {
        let mut overflow = used - total;
        for idx in (0..widths.len()).rev() {
            if overflow == 0 {
                break;
            }
            let reducible = widths[idx].saturating_sub(mins[idx]);
            let take = reducible.min(overflow);
            widths[idx] -= take;
            overflow -= take;
        }
        used = widths.iter().sum::<usize>() + separators;
    }
    if used < total {
        let extra = total - used;
        if let Some(last) = widths.last_mut() {
            *last += extra;
        }
    }
    widths
}

fn fit_left(text: &str, width: usize) -> String {
    fit(text, width, false)
}

fn fit_right(text: &str, width: usize) -> String {
    fit(text, width, true)
}

fn fit(text: &str, width: usize, right_align: bool) -> String {
    if width == 0 {
        return String::new();
    }
    let len = text.chars().count();
    if len == width {
        return text.to_string();
    }
    if len > width {
        if width == 1 {
            return "…".into();
        }
        let mut out = text.chars().take(width - 1).collect::<String>();
        out.push('…');
        return out;
    }
    let pad = " ".repeat(width - len);
    if right_align {
        format!("{pad}{text}")
    } else {
        format!("{text}{pad}")
    }
}

fn roster_column_widths(width: usize) -> Vec<usize> {
    // For the roster, the ROLE column is the primary piece of information (especially when
    // listing model slugs). Keep MISSION readable, but bias extra width to ROLE instead of letting
    // it all pool in the last column.
    let cols_total = width.saturating_sub(1);
    // +2 ROLE / -2 MISSION vs prior sizing so `computational-research` fits more comfortably.
    let mut widths = allocate_columns(cols_total, &[4, 6, 2, 1, 7], &[28, 10, 4, 2, 10], 4);

    // `allocate_columns` gives any extra space to the last column (MISSION). Shift surplus back to
    // ROLE so long model slugs don't get truncated while the right side sits empty.
    let mission_cap = 10usize;
    if widths.len() == 5 && widths[4] > mission_cap {
        let extra = widths[4].saturating_sub(mission_cap);
        widths[4] = widths[4].saturating_sub(extra);
        widths[0] = widths[0].saturating_add(extra);
    }

    widths
}

fn wrap_cell_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }

    let mut out: Vec<String> = Vec::new();
    let text = text.trim_end_matches(['\n', '\r']);
    for segment in text.split('\n') {
        let mut remaining = segment.trim_end_matches('\r');
        if remaining.is_empty() {
            out.push(String::new());
            continue;
        }

        loop {
            let len_chars = remaining.chars().count();
            if len_chars <= width {
                out.push(remaining.to_string());
                break;
            }

            let (prefix, rest) = split_at_chars(remaining, width);
            let Some((ws_idx, _)) = prefix.char_indices().rfind(|(_, ch)| ch.is_whitespace())
            else {
                out.push(prefix.to_string());
                remaining = rest;
                continue;
            };

            if ws_idx == 0 {
                out.push(prefix.to_string());
                remaining = rest.trim_start();
                continue;
            }

            let (line, tail) = remaining.split_at(ws_idx);
            out.push(line.trim_end().to_string());
            remaining = tail.trim_start();
        }
    }

    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn split_at_chars(text: &str, count: usize) -> (&str, &str) {
    if count == 0 {
        return ("", text);
    }
    let idx = text
        .char_indices()
        .nth(count)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    (&text[..idx], &text[idx..])
}

#[cfg(test)]
#[path = "tests/agent_ops_view.rs"]
mod tests;
