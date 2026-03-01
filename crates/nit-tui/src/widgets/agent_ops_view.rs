use nit_core::{
    AgentAlertSeverity, AgentOpsTab, AgentStatus, AppState, McpConnectionState, PaneId,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::theme::Theme;

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

pub fn render(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
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

    let rows = current_lines_for_width(state, chunks[1].width as usize);
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

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled("Keys: ", label_style));

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
        AgentOpsTab::Diagnostics => {
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
    let tabs = [
        AgentOpsTab::Roster,
        AgentOpsTab::Missions,
        AgentOpsTab::Mcp,
        AgentOpsTab::Alerts,
        AgentOpsTab::Diagnostics,
        AgentOpsTab::Scratchpad,
    ];
    let active = match state.agents.dock_tab {
        AgentOpsTab::Patch | AgentOpsTab::Evidence => AgentOpsTab::Diagnostics,
        other => other,
    };
    let mut spans = Vec::new();
    for (idx, tab) in tabs.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::raw("  "));
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

pub fn current_lines(state: &AppState) -> Vec<String> {
    current_lines_for_width(state, usize::MAX)
}

pub fn current_lines_for_width(state: &AppState, width: usize) -> Vec<String> {
    let usable = width.max(32);
    match state.agents.dock_tab {
        AgentOpsTab::Roster => roster_lines(state, usable),
        AgentOpsTab::Missions => mission_lines(state, usable),
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
    let mut out = vec![
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

fn selected_row_style(style: Style, selected: bool, theme: &Theme) -> Style {
    if selected {
        style.bg(theme.selection_bg).add_modifier(Modifier::BOLD)
    } else {
        style
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
            .fg(theme.accent)
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
    if state.agents.agents.is_empty() {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ));
    }
    let body_line = line_idx.saturating_sub(2);
    let Some(meta) = roster_body_meta(state, body_line) else {
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
                cols.get(0).cloned().unwrap_or_default(),
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
                cols.get(0).cloned().unwrap_or_default(),
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

    let marker_style = if selected && meta.agent_row == Some(0) {
        selected_row_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
            true,
            theme,
        )
    } else if selected {
        selected_row_style(
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
            true,
            theme,
        )
    } else {
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM)
    };
    let muted_style = selected_row_style(
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        selected,
        theme,
    );
    let is_primary_line = meta.agent_row == Some(0);
    let id_style = if is_primary_line {
        selected_row_style(Style::default().fg(theme.foreground), selected, theme)
    } else {
        muted_style
    };
    let phase_style = if is_primary_line {
        selected_row_style(mission_phase_style(mission.phase, theme), selected, theme)
    } else {
        muted_style
    };
    let swarm_style = if is_primary_line {
        selected_row_style(
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
            theme,
        )
    } else {
        muted_style
    };
    let status_style = if is_primary_line {
        selected_row_style(
            mission_status_style(&mission.status, theme),
            selected,
            theme,
        )
    } else {
        muted_style
    };
    let space_style = selected_row_style(Style::default(), selected, theme);

    let agent_edges = selected_row_style(
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        selected,
        theme,
    );
    let agent_assigned = selected_row_style(
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
        selected,
        theme,
    );
    let agent_missing = selected_row_style(
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        selected,
        theme,
    );

    let mut spans = Vec::with_capacity(20);
    spans.push(Span::styled(marker, marker_style));
    spans.push(Span::styled(
        cols.get(0).cloned().unwrap_or_default(),
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
        cols.get(0).cloned().unwrap_or_default(),
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
    {
        style = style.fg(theme.title_focused);
    } else if line.starts_with('+') && !line.starts_with("+++") {
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
    let text = text.trim_end_matches(|c| c == '\n' || c == '\r');
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
