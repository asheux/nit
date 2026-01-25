use nit_core::{AppState, PaneId};
use nit_gol::{AttractorEvent, Rule};
use ratatui::{
    layout::Constraint,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, BorderType, Borders, Cell, Row, Table},
    Frame,
};

use crate::theme::Theme;

pub fn render(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    syntax_status: &str,
) {
    let focused = state.focus == PaneId::GateMonitor;
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
        .border_style(border_style)
        .border_type(border_type)
        .style(Style::default().bg(theme.background))
        .title(Span::styled(
            "GATE MONITOR",
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    let (ln, col) = state.line_col();
    let viz_mode = match state.visualizer.mode {
        nit_core::VisualizerMode::SimOnly => "SIM",
        nit_core::VisualizerMode::Search => "SEARCH",
    };
    let search_intensity = format!("{:?}", state.settings.gol.search.intensity);
    let viz_period = state
        .visualizer
        .period
        .map(|p| p.to_string())
        .unwrap_or_else(|| "--".into());
    let viz_autostop = state.visualizer.auto_stop_policy.to_string();
    let viz_attractor = attractor_detail(state.visualizer.last_attractor.as_ref());
    let viz_pause_reason = if state.visualizer.paused {
        if state.visualizer.paused_by_attractor {
            "Attractor"
        } else {
            "User"
        }
    } else {
        "No"
    };
    let last_snapshot = state
        .visualizer
        .last_snapshot_path
        .as_deref()
        .map(|path| shorten_text(path, 30))
        .unwrap_or_else(|| "--".into());
    let rows = vec![
        Row::new(vec![Cell::from("Focus"), Cell::from(state.focus.title())]),
        Row::new(vec![
            Cell::from("Mode"),
            Cell::from(format!("{:?}", state.mode)),
        ]),
        Row::new(vec![
            Cell::from("Dirty"),
            Cell::from(if state.editor_buffer().is_dirty() {
                "Y"
            } else {
                "N"
            }),
        ]),
        Row::new(vec![
            Cell::from("Debug"),
            Cell::from(if state.debug { "ON" } else { "OFF" }),
        ]),
        Row::new(vec![
            Cell::from("Ln/Col"),
            Cell::from(format!("{ln}/{col}")),
        ]),
        Row::new(vec![
            Cell::from("Bytes"),
            Cell::from(state.editor_buffer().bytes_len().to_string()),
        ]),
        Row::new(vec![
            Cell::from("Render(ms)"),
            Cell::from(state.metrics.last_render_ms.to_string()),
        ]),
        Row::new(vec![
            Cell::from("Frame"),
            Cell::from(state.metrics.frame_count.to_string()),
        ]),
        Row::new(vec![
            Cell::from("Workspace"),
            Cell::from(shorten_path(&state.workspace_root, 30)),
        ]),
        Row::new(vec![
            Cell::from("Viz Mode"),
            Cell::from(viz_mode),
        ]),
        Row::new(vec![
            Cell::from("Seed Source"),
            Cell::from(format!("{:?}", state.visualizer.seed_source)),
        ]),
        Row::new(vec![
            Cell::from("Search Int"),
            Cell::from(search_intensity),
        ]),
        Row::new(vec![
            Cell::from("Viz Rule"),
            Cell::from(state.visualizer.rule.clone()),
        ]),
        Row::new(vec![
            Cell::from("Rule Bits"),
            Cell::from(rule_bits(&state.visualizer.rule)),
        ]),
        Row::new(vec![
            Cell::from("Viz Gen"),
            Cell::from(state.visualizer.generation.to_string()),
        ]),
        Row::new(vec![
            Cell::from("Viz Alive"),
            Cell::from(state.visualizer.alive.to_string()),
        ]),
        Row::new(vec![
            Cell::from("Viz Period"),
            Cell::from(viz_period),
        ]),
        Row::new(vec![
            Cell::from("Viz AutoStop"),
            Cell::from(viz_autostop),
        ]),
        Row::new(vec![
            Cell::from("Viz Attractor"),
            Cell::from(viz_attractor),
        ]),
        Row::new(vec![
            Cell::from("Viz PausedBy"),
            Cell::from(viz_pause_reason),
        ]),
        Row::new(vec![
            Cell::from("Viz Wrap"),
            Cell::from(if state.visualizer.wrap { "Torus" } else { "Dead" }),
        ]),
        Row::new(vec![
            Cell::from("Viz Speed"),
            Cell::from(format!("{}ms", state.visualizer.tick_ms)),
        ]),
        Row::new(vec![
            Cell::from("Search RPS"),
            Cell::from(state.visualizer.search_rps.to_string()),
        ]),
        Row::new(vec![
            Cell::from("Viz Seed"),
            Cell::from(state.visualizer.seed.to_string()),
        ]),
        Row::new(vec![
            Cell::from("Snap Written"),
            Cell::from(state.visualizer.snapshots_written.to_string()),
        ]),
        Row::new(vec![
            Cell::from("Snap Dropped"),
            Cell::from(state.visualizer.snapshots_dropped.to_string()),
        ]),
        Row::new(vec![
            Cell::from("Snap Queue"),
            Cell::from(state.visualizer.snapshot_queue_depth.to_string()),
        ]),
        Row::new(vec![Cell::from("Snap Last"), Cell::from(last_snapshot)]),
        Row::new(vec![Cell::from("Syntax"), Cell::from(syntax_status)]),
        Row::new(vec![
            Cell::from("Job paused"),
            Cell::from(format!("{}", state.job.paused)),
        ]),
    ];

    let mut rows = rows;
    for (idx, entry) in state.visualizer.leaderboard.iter().take(3).enumerate() {
        rows.push(Row::new(vec![
            Cell::from(format!("Rule {}", idx + 1)),
            Cell::from(format!("{} ({:.1})", entry.rule, entry.score)),
        ]));
    }

    let table = Table::new(rows, [Constraint::Length(14), Constraint::Min(5)])
        .column_spacing(1)
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));

    frame.render_widget(block, area);
    frame.render_widget(table, inner);
}

fn shorten_path(path: &std::path::Path, max: usize) -> String {
    let s = path.display().to_string();
    if s.len() <= max {
        s
    } else {
        format!("…{}", &s[s.len() - (max - 1)..])
    }
}

fn shorten_text(text: &str, max: usize) -> String {
    if text.len() <= max {
        text.to_string()
    } else {
        format!("…{}", &text[text.len() - (max - 1)..])
    }
}

fn rule_bits(rule_text: &str) -> String {
    let Ok(rule) = Rule::parse(rule_text) else {
        return "invalid".into();
    };
    let births = mask_bits(rule.births_mask());
    let survives = mask_bits(rule.survives_mask());
    format!("B:{births} S:{survives}")
}

fn mask_bits(mask: u16) -> String {
    let mut out = String::with_capacity(9);
    for i in 0..=8u8 {
        if (mask & (1 << i)) != 0 {
            out.push('1');
        } else {
            out.push('0');
        }
    }
    out
}

fn attractor_detail(event: Option<&AttractorEvent>) -> String {
    match event {
        Some(AttractorEvent::FixedPoint { gen }) => format!("Fixed @gen={gen}"),
        Some(AttractorEvent::Cycle {
            gen,
            period,
            transient,
            ..
        }) => format!("Cycle p={period} t={transient} @gen={gen}"),
        None => "--".into(),
    }
}
