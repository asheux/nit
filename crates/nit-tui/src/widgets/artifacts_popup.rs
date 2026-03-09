use nit_core::{AgentMessage, AppState, UiSelectionPane};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::swarm::SwarmRuntime;
use crate::theme::Theme;
use crate::widgets::agent_console_view;
use crate::widgets::agent_ops_view;
use crate::widgets::text_selection::apply_ui_selection;

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = (screen.width.saturating_sub(4)).clamp(40, 140);
    let height = (screen.height.saturating_sub(4)).clamp(12, 44);
    (width, height)
}

pub fn build_lines(
    state: &AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
    width: u16,
) -> Vec<Line<'static>> {
    let width_usize = width.max(1) as usize;
    if let Some(agent_ops_view::ArtifactsPopupRef::Message { idx }) =
        agent_ops_view::artifacts_popup_ref(state, swarm, width_usize)
    {
        if let Some(message) = state.agents.messages.get(idx) {
            return build_message_lines(state, message, theme, width_usize);
        }
    }

    let strings = agent_ops_view::artifacts_popup_strings(state, swarm, width_usize);
    strings
        .into_iter()
        .enumerate()
        .map(|(idx, line)| style_line(idx, line, theme))
        .collect()
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title(Span::styled(
            "ARTIFACT",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(theme.background).fg(theme.foreground));

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1)])
        .split(block.inner(area))[0];
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let lines = build_lines(state, swarm, theme, inner.width);
    let height = inner.height as usize;
    let max_scroll = lines.len().saturating_sub(height);
    let scroll = state.agents.artifacts_popup_scroll.min(max_scroll);
    let visible: Vec<Line> = lines.into_iter().skip(scroll).take(height).collect();
    let visible = apply_ui_selection(
        visible,
        state.ui_selection.as_ref(),
        UiSelectionPane::ArtifactsPopup,
        theme.cursor_line_bg,
        scroll,
    );

    let para = Paragraph::new(visible).style(Style::default().bg(theme.background));
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(para, inner);
}

fn build_message_lines(
    state: &AppState,
    message: &AgentMessage,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let kind = if message.agent_id.is_some() {
        "REPLY"
    } else {
        "PROMPT"
    };
    let owner = message.agent_id.as_deref().unwrap_or("You");
    let at = if message.at.trim().is_empty() {
        "--"
    } else {
        message.at.as_str()
    };
    let rule = "─".repeat(width.min(240));

    let mut out = Vec::new();
    out.push(Line::from(Span::styled(
        format!(" {kind}  {owner}  {at}"),
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
    )));
    out.push(Line::from(Span::styled(
        rule.clone(),
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
    )));

    let label_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);
    out.push(kv_line(
        "at:",
        message.at.as_str(),
        label_style,
        value_style,
    ));
    out.push(kv_line(
        "mission:",
        message.mission_id.as_deref().unwrap_or("ad-hoc"),
        label_style,
        value_style,
    ));
    out.push(kv_line("agent:", owner, label_style, value_style));

    out.push(Line::from(Span::styled(
        rule,
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
    )));
    out.push(Line::from(Span::styled(
        " Content",
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
    )));

    out.extend(agent_console_view::message_lines_for_popup(
        state, message, theme, width,
    ));
    out
}

fn kv_line(label: &str, value: &str, label_style: Style, value_style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {label} "), label_style),
        Span::styled(value.to_string(), value_style),
    ])
}

fn style_line(line_idx: usize, line: String, theme: &Theme) -> Line<'static> {
    if line.is_empty() {
        return Line::from("");
    }
    if line_idx == 0 {
        return Line::from(Span::styled(
            line,
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if line.chars().all(|ch| ch == '─') {
        return Line::from(Span::styled(
            line,
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ));
    }
    if matches!(line.as_str(), " Content" | " Diff (excerpt)") || line.starts_with(" Output") {
        return Line::from(Span::styled(
            line,
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
        let value_trimmed = value.trim_start();
        let value_style =
            if value_trimmed.starts_with("http://") || value_trimmed.starts_with("https://") {
                Style::default()
                    .fg(theme.hl.link)
                    .add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default().fg(theme.foreground)
            };
        return Line::from(vec![
            Span::styled(
                label_text,
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM),
            ),
            Span::styled(value.to_string(), value_style),
        ]);
    }

    Line::from(Span::styled(line, Style::default().fg(theme.foreground)))
}
