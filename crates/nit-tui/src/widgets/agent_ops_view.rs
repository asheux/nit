use nit_core::{
    AgentAlertSeverity, AgentLaneKind, AgentOpsTab, AgentStatus, AppState, McpConnectionState,
    PaneId, UiSelectionPane,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::swarm::{SwarmDashboardView, SwarmRuntime};
use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;

pub fn mission_index_for_body_line(state: &AppState, body_line: usize) -> Option<usize> {
    mission_body_meta(state, body_line).map(|meta| meta.mission_idx)
}

pub struct RosterBodyMeta {
    pub agent_idx: usize,
    pub effort_idx: Option<usize>,
}

pub fn roster_meta_for_body_line(state: &AppState, body_line: usize) -> Option<RosterBodyMeta> {
    roster_body_meta(state, body_line)
}

pub fn roster_body_offset(state: &AppState) -> usize {
    let _ = state;
    // Backends header (4 lines) + blank spacer (1) + swarm template buttons (1) + table
    // header/separator (2)
    8
}

pub const ROSTER_SWARM_TEMPLATE_LINE_IDX: usize = 5;

const ROSTER_SWARM_TEMPLATE_LINE: &str = " Swarm template:  lab   parallel   bulk ";

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

pub fn alert_index_for_body_line(
    state: &AppState,
    width: usize,
    body_line: usize,
) -> Option<usize> {
    alert_body_meta(state, width, body_line).map(|meta| meta.alert_idx)
}

fn roster_body_meta(state: &AppState, body_line: usize) -> Option<RosterBodyMeta> {
    let mut cursor = 0usize;
    for (agent_idx, agent) in state.agents.agents.iter().enumerate() {
        if body_line == cursor {
            return Some(RosterBodyMeta {
                agent_idx,
                effort_idx: None,
            });
        }
        cursor = cursor.saturating_add(1);
        if agent_idx == state.agents.roster_selected {
            let effort_len = state
                .agents
                .codex_supported_reasoning_efforts
                .get(&agent.id)
                .map(|v| v.len())
                .unwrap_or(0);
            if body_line < cursor.saturating_add(effort_len) {
                return Some(RosterBodyMeta {
                    agent_idx,
                    effort_idx: Some(body_line.saturating_sub(cursor)),
                });
            }
            cursor = cursor.saturating_add(effort_len);
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
            Constraint::Min(1),    // body
            Constraint::Length(1), // footer hints
        ])
        .split(inner);

    render_tab_bar(frame, chunks[0], state, theme);

    let rows = current_lines_for_width_with_swarm(state, Some(swarm), chunks[1].width as usize);
    let height = chunks[1].height as usize;
    let max_scroll = rows.len().saturating_sub(height);
    let scroll = state.agents.ops_scroll.min(max_scroll);
    let visible = rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(height)
        .map(|(idx, line)| ops_styled_line(state, idx, line, chunks[1].width as usize, theme))
        .collect::<Vec<_>>();
    let visible = apply_ui_selection(
        visible,
        state.ui_selection.as_ref(),
        UiSelectionPane::JobOutput,
        theme.selection_bg,
        scroll,
    );
    frame.render_widget(
        Paragraph::new(visible).style(Style::default().bg(theme.background)),
        chunks[1],
    );

    render_footer(frame, chunks[2], state, theme);
}

pub fn render_tab_bar(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let tabs = tab_line(state, theme);
    frame.render_widget(Paragraph::new(tabs), area);
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
            spans.push(Span::styled("1/2/3", key_style));
            spans.push(Span::styled(" template", label_style));
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
        // Patch/Evidence are legacy; render them like Diagnostics.
        AgentOpsTab::Patch | AgentOpsTab::Evidence => {
            spans.push(Span::styled("j/k", key_style));
            spans.push(Span::styled(" scroll", label_style));
        }
    }

    Line::from(spans)
}

fn tab_line(state: &AppState, theme: &Theme) -> Line<'static> {
    const TABS: [AgentOpsTab; 7] = [
        AgentOpsTab::Roster,
        AgentOpsTab::Missions,
        AgentOpsTab::Dag,
        AgentOpsTab::Mcp,
        AgentOpsTab::Alerts,
        AgentOpsTab::Diagnostics,
        AgentOpsTab::Scratchpad,
    ];
    const TAB_SPACING: &str = "  ";
    let active = match state.agents.dock_tab {
        AgentOpsTab::Patch | AgentOpsTab::Evidence => AgentOpsTab::Diagnostics,
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
    const TABS: [AgentOpsTab; 7] = [
        AgentOpsTab::Roster,
        AgentOpsTab::Missions,
        AgentOpsTab::Dag,
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
        AgentOpsTab::Roster => roster_lines(state, usable),
        AgentOpsTab::Missions => mission_lines(state, usable),
        AgentOpsTab::Dag => dag_lines(state, swarm, usable),
        AgentOpsTab::Mcp => mcp_lines(state, usable),
        AgentOpsTab::Alerts => alert_lines(state, usable),
        // Patch/Evidence are hidden from the UI; treat as Diagnostics for legacy state.
        AgentOpsTab::Patch | AgentOpsTab::Evidence => diagnostics_lines(state, usable),
        AgentOpsTab::Diagnostics => diagnostics_lines(state, usable),
        AgentOpsTab::Scratchpad => scratchpad_lines(state, usable),
    }
}

fn roster_lines(state: &AppState, width: usize) -> Vec<String> {
    let widths = roster_column_widths(width);
    let codex_active = state.agents.agents.iter().any(|agent| agent.is_codex());
    let local_active = state
        .agents
        .agents
        .iter()
        .any(|agent| matches!(agent.kind, AgentLaneKind::Mock));
    let codex_available = state.agents.codex_cli_available || codex_active;
    let claude_active = state
        .agents
        .agents
        .iter()
        .any(|agent| matches!(agent.kind, AgentLaneKind::Claude));
    let claude_available = state.agents.claude_cli_available || claude_active;

    let mut out = vec![
        " Backends".into(),
        format!(
            "  Codex  {}{}",
            if codex_available {
                "available"
            } else {
                "not found"
            },
            if codex_active { " (active)" } else { "" }
        ),
        format!(
            "  Claude {}{}",
            if claude_available {
                "available"
            } else {
                "not found"
            },
            if claude_active { " (active)" } else { "" }
        ),
        format!(
            "  Local  built-in{}",
            if local_active { " (active)" } else { "" }
        ),
        String::new(),
        ROSTER_SWARM_TEMPLATE_LINE.into(),
        format!(
            " {} {} {} {} {}",
            fit_left("ROLE", widths[0]),
            fit_left("STATUS", widths[1]),
            fit_right("HB", widths[2]),
            fit_right("Q", widths[3]),
            fit_left("MISSION", widths[4]),
        ),
        "─".repeat(width.min(240)),
    ];

    if state.agents.agents.is_empty() {
        out.push(" No agents available.".into());
        return out;
    }
    for (idx, agent) in state.agents.agents.iter().enumerate() {
        let marker = if idx == state.agents.roster_selected
            && state.agents.roster_effort_selected.is_none()
        {
            ">"
        } else {
            " "
        };
        out.push(format!(
            "{marker}{} {} {} {} {}",
            fit_left(&agent.role, widths[0]),
            fit_left(agent.status.label(), widths[1]),
            fit_right(&format!("{}s", agent.heartbeat_age_secs), widths[2]),
            fit_right(&agent.queue_len.to_string(), widths[3]),
            fit_left(agent.current_mission.as_deref().unwrap_or("--"), widths[4]),
        ));

        // Expand the selected model into a "size" tree (Codex reasoning effort levels).
        if idx == state.agents.roster_selected {
            let efforts = state
                .agents
                .codex_supported_reasoning_efforts
                .get(&agent.id)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            if efforts.is_empty() {
                continue;
            }
            let chosen = state
                .agents
                .codex_selected_reasoning_effort
                .get(&agent.id)
                .or_else(|| state.agents.codex_default_reasoning_effort.get(&agent.id))
                .map(|s| s.as_str());
            for (effort_idx, effort) in efforts.iter().enumerate() {
                let marker = if state.agents.roster_effort_selected == Some(effort_idx) {
                    ">"
                } else {
                    " "
                };
                let branch = if effort_idx + 1 == efforts.len() {
                    "└"
                } else {
                    "├"
                };
                let checked = if chosen == Some(effort.as_str()) {
                    "*"
                } else {
                    " "
                };
                let label = format!("  {branch}─ [{checked}] {effort}");
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
            let marker = if idx == state.agents.mission_selected && agent_idx == 0 {
                ">"
            } else {
                " "
            };
            let agent_label = mission
                .assigned_agents
                .get(agent_idx)
                .map(|s| s.as_str())
                .unwrap_or("--");
            if agent_idx == 0 {
                out.push(format!(
                    "{marker}{} {} {} {} {}",
                    fit_left(&mission.id, widths[0]),
                    fit_left(mission.phase.label(), widths[1]),
                    fit_left(if mission.swarm { "yes" } else { "no" }, widths[2]),
                    fit_left(&mission.status, widths[3]),
                    agent_pane_mid(agent_label, widths[4]),
                ));
            } else {
                out.push(format!(
                    "{marker}{} {} {} {} {}",
                    empty0,
                    empty1,
                    empty2,
                    empty3,
                    agent_pane_mid(agent_label, widths[4]),
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
            let marker = if row == 0 && selected { ">" } else { " " };
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
    let summary = format!(
        "Swarm DAG {} [{status_word}] template={} phase={} done={}/{} failed={} skipped={} running={} queued={} pending={}",
        dashboard.mission_id,
        dashboard.template,
        dashboard.phase,
        dashboard.done,
        total,
        dashboard.failed,
        dashboard.skipped,
        dashboard.running,
        dashboard.queued,
        dashboard.pending
    );
    for chunk in wrap_cell_text(&summary, cols_total) {
        out.push(format!(" {chunk}"));
    }
    if let Some(bundle) = dashboard.gate_bundle.as_deref() {
        let line = format!("Gate bundle: {bundle}");
        for chunk in wrap_cell_text(&line, cols_total) {
            out.push(format!(" {chunk}"));
        }
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
            let role = task.role.as_deref().unwrap_or("-");
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
                out.push(format!(
                    " {} {} {}",
                    fit_left(id_cell, task_widths[0]),
                    fit_left(state_cell, task_widths[1]),
                    fit_left(chunk, task_widths[2]),
                ));
            }

            let details_1 = format!("agent: {}  role: {}", task.agent_id, role);
            let details_1_chunks = wrap_cell_text(&details_1, task_widths[2].saturating_sub(2));
            for chunk in details_1_chunks {
                let line = format!("↳ {chunk}");
                out.push(format!(
                    " {} {} {}",
                    empty_id,
                    empty_state,
                    fit_left(&line, task_widths[2]),
                ));
            }

            let details_2 = format!(
                "deps: {}  block: {}  writes: {}  out: {}  done_when: {}",
                deps, blocked, writes, out_present, done_when
            );
            let details_2_chunks = wrap_cell_text(&details_2, task_widths[2].saturating_sub(2));
            for chunk in details_2_chunks {
                let line = format!("↳ {chunk}");
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
                let line = format!("↳ {chunk}");
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

fn diagnostics_lines(state: &AppState, width: usize) -> Vec<String> {
    let mut out = vec![" DIAGNOSTICS".into(), "─".repeat(width.min(240))];
    for event in &state.agents.diag_events {
        out.push(format!(
            "[{}] [{}] {} - {}",
            event.at,
            event.severity.label(),
            event.source,
            fit_left(&event.message, width.saturating_sub(24))
        ));
    }
    let mut logs = state.logs.iter().cloned().collect::<Vec<_>>();
    if logs.len() > 32 {
        logs = logs.split_off(logs.len() - 32);
    }
    if !logs.is_empty() {
        out.push(String::new());
        out.push(" Runtime log tail".into());
        out.push(" ────────────────".into());
        for line in logs {
            out.push(fit_left(&line, width.saturating_sub(1)));
        }
    }
    out
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
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ));
    }
    if line_idx == 1 {
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
        // Patch/Evidence are hidden from the UI; render them like Diagnostics for legacy state.
        AgentOpsTab::Patch | AgentOpsTab::Evidence | AgentOpsTab::Diagnostics => Line::from(
            Span::styled(line.to_string(), ops_line_style(line_idx, line, theme)),
        ),
        AgentOpsTab::Scratchpad => Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        )),
    }
}

fn dag_table_bg(theme: &Theme) -> Color {
    dim_bg_towards(theme.border, theme.background, 85)
}

fn dag_state_style(state: &str, base: Style, theme: &Theme) -> Style {
    match state {
        "Running" => base.fg(theme.title_focused).add_modifier(Modifier::BOLD),
        "Queued" | "Pending" => base.fg(theme.accent).add_modifier(Modifier::BOLD),
        "Failed" => base.fg(theme.error).add_modifier(Modifier::BOLD),
        "Done" | "Skipped" => base.fg(theme.border).add_modifier(Modifier::DIM),
        _ => base.fg(theme.foreground),
    }
}

fn dag_gate_status_style(status: &str, base: Style, theme: &Theme) -> Style {
    match status {
        "PASS" => base.fg(theme.title_focused).add_modifier(Modifier::BOLD),
        "FAIL" => base.fg(theme.error).add_modifier(Modifier::BOLD),
        "PENDING" => base.fg(theme.border).add_modifier(Modifier::DIM),
        _ => base.fg(theme.foreground),
    }
}

fn dag_styled_line(_line_idx: usize, line: &str, usable: usize, theme: &Theme) -> Line<'static> {
    let cols_total = usable.saturating_sub(1);
    let bg = dag_table_bg(theme);
    let base = Style::default().bg(bg);
    let header_style = base
        .fg(theme.border)
        .add_modifier(Modifier::DIM | Modifier::BOLD);
    let row_style = base.fg(theme.foreground);
    let dim_row_style = base.fg(theme.border).add_modifier(Modifier::DIM);

    let trimmed = line.trim_start();
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
            let gate_style = row_style
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD);
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
        && title_cell.trim_start().starts_with('↳');
    let marker_style = row_style.fg(theme.border).add_modifier(Modifier::DIM);
    let id_style = if is_details {
        dim_row_style
    } else {
        row_style
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD)
    };
    let state_style = if is_details {
        dim_row_style
    } else {
        dag_state_style(state_cell.trim(), row_style, theme)
    };
    let title_style = if is_details {
        row_style.add_modifier(Modifier::DIM)
    } else {
        row_style.add_modifier(Modifier::BOLD)
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
        style.bg(theme.selection_bg).add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn striped_row_style(style: Style, selected: bool, striped: bool, theme: &Theme) -> Style {
    if selected {
        style.bg(theme.selection_bg).add_modifier(Modifier::BOLD)
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
        AgentStatus::Running => Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
        AgentStatus::Waiting => Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        AgentStatus::Idle => Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        AgentStatus::Error => Style::default()
            .fg(theme.error)
            .add_modifier(Modifier::BOLD),
    }
}

fn heartbeat_age_style(age_secs: u64, theme: &Theme) -> Style {
    if age_secs <= 2 {
        Style::default().fg(theme.title_focused)
    } else if age_secs <= 5 {
        Style::default().fg(theme.warning)
    } else {
        Style::default()
            .fg(theme.error)
            .add_modifier(Modifier::BOLD)
    }
}

fn queue_len_style(queue_len: usize, theme: &Theme) -> Style {
    if queue_len > 0 {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM)
    }
}

fn alert_severity_style(severity: AgentAlertSeverity, theme: &Theme) -> Style {
    match severity {
        AgentAlertSeverity::Info => Style::default().fg(theme.title_focused),
        AgentAlertSeverity::Warn => Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD),
        AgentAlertSeverity::Error => Style::default()
            .fg(theme.error)
            .add_modifier(Modifier::BOLD),
    }
}

fn mcp_state_style(state: McpConnectionState, theme: &Theme) -> Style {
    match state {
        McpConnectionState::Connected => Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
        McpConnectionState::Connecting => Style::default()
            .fg(theme.hl.operator)
            .add_modifier(Modifier::BOLD),
        McpConnectionState::Disconnected => Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        McpConnectionState::Error => Style::default()
            .fg(theme.error)
            .add_modifier(Modifier::BOLD),
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

fn roster_styled_line(
    state: &AppState,
    line_idx: usize,
    line: &str,
    usable: usize,
    theme: &Theme,
) -> Line<'static> {
    if line_idx == 0 {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if line_idx == 1 {
        let codex_active = state.agents.agents.iter().any(|agent| agent.is_codex());
        let codex_available = state.agents.codex_cli_available || codex_active;
        let style = if codex_active {
            Style::default()
                .fg(theme.hl.operator)
                .add_modifier(Modifier::BOLD)
        } else if codex_available {
            Style::default().fg(theme.foreground)
        } else {
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM)
        };
        return Line::from(Span::styled(line.to_string(), style));
    }
    if line_idx == 2 {
        let claude_active = state
            .agents
            .agents
            .iter()
            .any(|agent| matches!(agent.kind, AgentLaneKind::Claude));
        let claude_available = state.agents.claude_cli_available || claude_active;
        let style = if claude_active {
            Style::default()
                .fg(theme.hl.operator)
                .add_modifier(Modifier::BOLD)
        } else if claude_available {
            Style::default().fg(theme.foreground)
        } else {
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM)
        };
        return Line::from(Span::styled(line.to_string(), style));
    }
    if line_idx == 3 {
        let local_active = state
            .agents
            .agents
            .iter()
            .any(|agent| matches!(agent.kind, AgentLaneKind::Mock));
        let style = if local_active {
            Style::default()
                .fg(theme.seed.accent_2)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };
        return Line::from(Span::styled(line.to_string(), style));
    }
    if line_idx == 4 {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.foreground),
        ));
    }
    if line_idx == 5 {
        let label_style = Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM);
        let selected_style = Style::default()
            .fg(theme.background)
            .bg(theme.border_focused)
            .add_modifier(Modifier::BOLD);
        let unselected_style = Style::default()
            .fg(theme.foreground)
            .bg(theme.cursor_line_bg);

        let mut spans: Vec<Span<'static>> = Vec::with_capacity(8);
        spans.push(Span::styled(" Swarm template: ", label_style));
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
    if line_idx == 6 {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if line_idx == 7 {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
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
    let Some(agent) = state.agents.agents.get(meta.agent_idx) else {
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
    match meta.effort_idx {
        None => {
            let selected = meta.agent_idx == state.agents.roster_selected
                && state.agents.roster_effort_selected.is_none();

            let marker_style = if selected {
                selected_row_style(
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                    true,
                    theme,
                )
            } else {
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM)
            };

            let role_style =
                selected_row_style(Style::default().fg(theme.foreground), selected, theme);
            let status_style =
                selected_row_style(agent_status_style(agent.status, theme), selected, theme);
            let hb_style = selected_row_style(
                heartbeat_age_style(agent.heartbeat_age_secs, theme),
                selected,
                theme,
            );
            let q_style =
                selected_row_style(queue_len_style(agent.queue_len, theme), selected, theme);
            let mission_style = selected_row_style(
                if agent.current_mission.is_some() {
                    Style::default().fg(theme.title)
                } else {
                    Style::default()
                        .fg(theme.border)
                        .add_modifier(Modifier::DIM)
                },
                selected,
                theme,
            );
            let space_style = selected_row_style(Style::default(), selected, theme);

            let mut spans = Vec::with_capacity(14);
            spans.push(Span::styled(marker, marker_style));
            spans.push(Span::styled(
                cols.first().cloned().unwrap_or_default(),
                role_style,
            ));
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
        Some(effort_idx) => {
            let selected = meta.agent_idx == state.agents.roster_selected
                && state.agents.roster_effort_selected == Some(effort_idx);
            let chosen = state
                .agents
                .codex_selected_reasoning_effort
                .get(&agent.id)
                .or_else(|| state.agents.codex_default_reasoning_effort.get(&agent.id))
                .map(|s| s.as_str());
            let effort = state
                .agents
                .codex_supported_reasoning_efforts
                .get(&agent.id)
                .and_then(|v| v.get(effort_idx))
                .map(|s| s.as_str());
            let is_chosen = effort.is_some_and(|effort| chosen == Some(effort));

            let marker_style = if selected {
                selected_row_style(
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                    true,
                    theme,
                )
            } else {
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM)
            };

            let base_role_style = if is_chosen {
                Style::default()
                    .fg(theme.title_focused)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM)
            };
            let role_style = selected_row_style(base_role_style, selected, theme);
            let cell_style = selected_row_style(Style::default(), selected, theme);

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
        Style::default()
            .fg(theme.error)
            .add_modifier(Modifier::BOLD)
    } else if upper.contains("WARN") {
        Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD)
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
        striped_row_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
            selected,
            striped,
            theme,
        )
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
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
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
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
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
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else if key.trim() == "LAST ERR" {
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD)
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
        selected_row_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
            true,
            theme,
        )
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
        return Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    }
    if line_idx == 1 {
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
        style = style.bg(theme.selection_bg).add_modifier(Modifier::BOLD);
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
    let mut widths = allocate_columns(cols_total, &[4, 6, 2, 1, 7], &[24, 10, 4, 2, 14], 4);

    // `allocate_columns` gives any extra space to the last column (MISSION). Shift surplus back to
    // ROLE so long model slugs don't get truncated while the right side sits empty.
    let mission_cap = 14usize;
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
mod tests {
    use super::dag_lines_for_dashboard;
    use crate::swarm::{SwarmDashboardView, SwarmGateDashboardRow, SwarmTaskDashboardRow};

    #[test]
    fn dag_lines_include_tasks_and_gates() {
        let dashboard = SwarmDashboardView {
            mission_id: "mis-009".into(),
            template: "plan-v2".into(),
            phase: "EXEC".into(),
            done: 1,
            failed: 0,
            skipped: 0,
            running: 1,
            queued: 0,
            pending: 0,
            tasks: vec![SwarmTaskDashboardRow {
                id: "t1".into(),
                title: "Integrate dashboard changes".into(),
                role: Some("integrator".into()),
                agent_id: "agent-1".into(),
                state: "Running".into(),
                deps: vec!["t0".into()],
                blocked_on: Vec::new(),
                writes: true,
                done_when: Some("UI matches spec".into()),
                output_present: false,
            }],
            gate_bundle: Some("rust-ci".into()),
            gates: vec![SwarmGateDashboardRow {
                name: "fmt".into(),
                command: "cargo fmt --all -- --check".into(),
                status: "PENDING".into(),
                notes: None,
            }],
        };

        let lines = dag_lines_for_dashboard(&dashboard, 80);
        assert!(lines.iter().any(|line| line.contains("Swarm DAG")));
        assert!(lines.iter().any(|line| line.contains("t1")));
        assert!(lines.iter().any(|line| line.contains("fmt")));
    }

    #[test]
    fn dag_lines_wrap_instead_of_ellipsis() {
        let dashboard = SwarmDashboardView {
            mission_id: "mis-010".into(),
            template: "plan-v2".into(),
            phase: "EXEC".into(),
            done: 0,
            failed: 0,
            skipped: 0,
            running: 1,
            queued: 0,
            pending: 0,
            tasks: vec![SwarmTaskDashboardRow {
                id: "t1".into(),
                title: "This is a very long title that should wrap across multiple lines".into(),
                role: Some("integrator".into()),
                agent_id: "agent-1".into(),
                state: "Running".into(),
                deps: vec!["t0".into(), "t2".into(), "t3".into(), "t4".into()],
                blocked_on: vec!["gate-fmt".into(), "gate-clippy".into()],
                writes: true,
                done_when: Some(
                    "Ensure the DAG view never truncates with ellipsis; wrap instead.".into(),
                ),
                output_present: false,
            }],
            gate_bundle: Some(
                "bundle-with-a-very-long-name-that-must-wrap-instead-of-truncating".into(),
            ),
            gates: vec![SwarmGateDashboardRow {
                name: "fmt".into(),
                command: "cargo fmt --all -- --check && echo \"hello world\" && echo \"more\""
                    .into(),
                status: "PENDING".into(),
                notes: None,
            }],
        };

        let lines = dag_lines_for_dashboard(&dashboard, 48);
        assert!(
            !lines.iter().any(|line| line.contains('…')),
            "expected DAG output to wrap without ellipsis"
        );
        assert!(
            lines.iter().any(|line| line.trim_end().ends_with('\\')),
            "expected wrapped commands to use backslash continuation"
        );
    }
}
