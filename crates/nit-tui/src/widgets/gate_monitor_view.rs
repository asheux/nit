use nit_core::seed::SeedViewMode;
use nit_core::{AppKind, AppState, GamesStatus, GolSearchIntensity, PaneId, RuleMode};
use nit_gol::{AttractorEvent, Rule};
use ratatui::{
    layout::Constraint,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, BorderType, Borders, Cell, Row, Table},
    Frame,
};

use crate::theme::Theme;

#[derive(Clone, Debug)]
pub struct SyntaxDebugInfo {
    pub buffer_version: u64,
    pub snapshot_version: Option<u64>,
    pub engine_state: String,
    pub last_job_ms: Option<u128>,
}

pub fn render(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    syntax_status: &str,
    syntax_debug: Option<SyntaxDebugInfo>,
) {
    if state.app_kind == AppKind::Games {
        return render_games(frame, area, state, theme, syntax_status, syntax_debug);
    }
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
    let petri_mode = match state.visualizer.mode {
        nit_core::VisualizerMode::SimOnly => "SIM",
        nit_core::VisualizerMode::Search => "SEARCH",
    };
    let search_intensity = format!("{:?}", state.settings.gol.search.intensity);
    let petri_period = state
        .visualizer
        .period
        .map(|p| p.to_string())
        .unwrap_or_else(|| "--".into());
    let petri_autostop = state.visualizer.auto_stop_policy.to_string();
    let petri_attractor = attractor_detail(state.visualizer.last_attractor.as_ref());
    let petri_pause_reason = if state.visualizer.paused {
        if state.visualizer.paused_by_attractor {
            "Attractor"
        } else {
            "User"
        }
    } else {
        "No"
    };
    let seed_hash = if state.visualizer.seed_hash == 0 {
        "--".into()
    } else {
        format!("{:08x}", state.visualizer.seed_hash as u32)
    };
    let seed_view = match state.visualizer.seed_view {
        SeedViewMode::Plate => {
            format!("PLATE/{}", state.visualizer.seed_plate_mode.label())
        }
        _ => state.visualizer.seed_view.label().to_string(),
    };
    let seed_last_snapshot = state
        .visualizer
        .seed_last_snapshot_path
        .as_deref()
        .map(|path| shorten_text(path, 30))
        .unwrap_or_else(|| "--".into());
    let sim_last_snapshot = state
        .visualizer
        .last_snapshot_path
        .as_deref()
        .map(|path| shorten_text(path, 30))
        .unwrap_or_else(|| "--".into());
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);

    let mut rows = Vec::new();
    rows.push(row(
        "Focus",
        state.focus.title().to_string(),
        label_style,
        Style::default().fg(theme.title_focused),
    ));
    rows.push(row(
        "Mode",
        format!("{:?}", state.mode),
        label_style,
        mode_style(state.mode, theme),
    ));
    rows.push(row(
        "Dirty",
        if state.editor_buffer().is_dirty() {
            "Y".to_string()
        } else {
            "N".to_string()
        },
        label_style,
        if state.editor_buffer().is_dirty() {
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Debug",
        if state.debug { "ON" } else { "OFF" }.to_string(),
        label_style,
        if state.debug {
            Style::default().fg(theme.warning)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Ln/Col",
        format!("{ln}/{col}"),
        label_style,
        value_style,
    ));
    rows.push(row(
        "Bytes",
        state.editor_buffer().bytes_len().to_string(),
        label_style,
        value_style,
    ));
    rows.push(row(
        "Render(ms)",
        state.metrics.last_render_ms.to_string(),
        label_style,
        render_ms_style(state.metrics.last_render_ms, theme),
    ));
    rows.push(row(
        "Frame",
        state.metrics.frame_count.to_string(),
        label_style,
        dim_style,
    ));
    rows.push(row(
        "Workspace",
        shorten_path(&state.workspace_root, 30),
        label_style,
        value_style,
    ));
    rows.push(row(
        "Seed Enc",
        state.visualizer.seed_encoder.label().to_string(),
        label_style,
        Style::default().fg(theme.title_focused),
    ));
    rows.push(row("Seed Hash", seed_hash, label_style, value_style));
    rows.push(row(
        "Seed Dens",
        format!("{:.2}", state.visualizer.seed_stats.density),
        label_style,
        value_style,
    ));
    rows.push(row(
        "Seed Sym",
        state.visualizer.seed_params.symmetry.label().to_string(),
        label_style,
        value_style,
    ));
    rows.push(row("Seed View", seed_view, label_style, value_style));
    rows.push(row(
        "Seed Source",
        format!("{:?}", state.visualizer.seed_source),
        label_style,
        match state.visualizer.seed_source {
            nit_core::GolSeedSource::Editor => Style::default().fg(theme.title),
            nit_core::GolSeedSource::Notes => Style::default().fg(theme.title_focused),
        },
    ));
    rows.push(row(
        "Seed Search",
        if state.visualizer.seed_search_active {
            "ON"
        } else {
            "OFF"
        }
        .to_string(),
        label_style,
        if state.visualizer.seed_search_active {
            Style::default().fg(theme.accent)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Seed Snap",
        state.visualizer.seed_snapshots_written.to_string(),
        label_style,
        if state.visualizer.seed_snapshots_written > 0 {
            Style::default().fg(theme.title)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Seed Drop",
        state.visualizer.seed_snapshots_dropped.to_string(),
        label_style,
        if state.visualizer.seed_snapshots_dropped > 0 {
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Seed Queue",
        state.visualizer.seed_snapshot_queue_depth.to_string(),
        label_style,
        if state.visualizer.seed_snapshot_queue_depth > 0 {
            Style::default().fg(theme.warning)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Seed Last",
        seed_last_snapshot,
        label_style,
        if state.visualizer.seed_last_snapshot_path.is_some() {
            Style::default().fg(theme.accent)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Petri Open",
        if state.visualizer.running { "Y" } else { "N" }.to_string(),
        label_style,
        if state.visualizer.running {
            Style::default().fg(theme.title_focused)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Petri Mode",
        petri_mode.to_string(),
        label_style,
        match state.visualizer.mode {
            nit_core::VisualizerMode::Search => Style::default().fg(theme.accent),
            nit_core::VisualizerMode::SimOnly => Style::default().fg(theme.title_focused),
        },
    ));
    rows.push(row(
        "Gol Rule",
        format!(
            "{} / {}",
            state
                .gol_rule_selected
                .id
                .clone()
                .unwrap_or_else(|| "custom".into()),
            state.gol_rule_selected.rule
        ),
        label_style,
        value_style,
    ));
    rows.push(row(
        "Search Int",
        search_intensity,
        label_style,
        intensity_style(state.settings.gol.search.intensity, theme),
    ));
    rows.push(row(
        "Petri Rule",
        state.visualizer.rule.clone(),
        label_style,
        value_style,
    ));
    rows.push(row(
        "Petri RuleName",
        rule_name(&state.visualizer.rule, &state.rule_catalog),
        label_style,
        dim_style,
    ));
    if let RuleMode::Protocol(protocol) = &state.visualizer.rule_mode {
        let proto_name = state
            .visualizer
            .protocol_name
            .clone()
            .unwrap_or_else(|| "Protocol".into());
        rows.push(row("Petri Proto", proto_name, label_style, value_style));
        let phase_label = protocol
            .current_phase()
            .label
            .clone()
            .unwrap_or_else(|| "Phase".into());
        let phase_text = format!(
            "{}/{} \"{}\" t={}/{}",
            protocol.phase_idx + 1,
            protocol.phase_count(),
            phase_label,
            protocol.step_in_phase + 1,
            protocol.current_phase().steps.max(1)
        );
        rows.push(row("Petri Phase", phase_text, label_style, dim_style));
    }
    rows.push(row(
        "Rule Bits",
        rule_bits(&state.visualizer.rule),
        label_style,
        dim_style,
    ));
    rows.push(row(
        "Petri Gen",
        state.visualizer.generation.to_string(),
        label_style,
        value_style,
    ));
    rows.push(row(
        "Petri Alive",
        state.visualizer.alive.to_string(),
        label_style,
        if state.visualizer.alive > 0 {
            Style::default().fg(theme.title_focused)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Petri Period",
        petri_period,
        label_style,
        if state.visualizer.period.is_some() {
            Style::default().fg(theme.title)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Petri Auto",
        petri_autostop,
        label_style,
        autostop_style(state.visualizer.auto_stop_policy, theme),
    ));
    rows.push(row(
        "Petri Attr",
        petri_attractor,
        label_style,
        attractor_style(state.visualizer.last_attractor.as_ref(), theme, dim_style),
    ));
    rows.push(row(
        "Petri Pause",
        petri_pause_reason.to_string(),
        label_style,
        paused_by_style(
            state.visualizer.paused,
            state.visualizer.paused_by_attractor,
            theme,
            dim_style,
        ),
    ));
    rows.push(row(
        "Petri Wrap",
        if state.visualizer.wrap {
            "Torus"
        } else {
            "Dead"
        }
        .to_string(),
        label_style,
        if state.visualizer.wrap {
            Style::default().fg(theme.title)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Petri Tick",
        format!("{}ms", state.visualizer.tick_ms),
        label_style,
        tick_ms_style(state.visualizer.tick_ms, theme),
    ));
    rows.push(row(
        "Search RPS",
        state.visualizer.search_rps.to_string(),
        label_style,
        if state.visualizer.search_rps > 0 {
            Style::default().fg(theme.accent)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Sim Snap",
        state.visualizer.snapshots_written.to_string(),
        label_style,
        if state.visualizer.snapshots_written > 0 {
            Style::default().fg(theme.title)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Sim Drop",
        state.visualizer.snapshots_dropped.to_string(),
        label_style,
        if state.visualizer.snapshots_dropped > 0 {
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Sim Queue",
        state.visualizer.snapshot_queue_depth.to_string(),
        label_style,
        if state.visualizer.snapshot_queue_depth > 0 {
            Style::default().fg(theme.warning)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Sim Last",
        sim_last_snapshot,
        label_style,
        if state.visualizer.last_snapshot_path.is_some() {
            Style::default().fg(theme.accent)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Syntax",
        syntax_status.to_string(),
        label_style,
        syntax_style(syntax_status, theme, dim_style),
    ));
    if state.debug {
        if let Some(debug) = syntax_debug {
            rows.push(row(
                "Buf Ver",
                debug.buffer_version.to_string(),
                label_style,
                value_style,
            ));
            rows.push(row(
                "HL Ver",
                debug
                    .snapshot_version
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "--".to_string()),
                label_style,
                value_style,
            ));
            rows.push(row(
                "HL Job(ms)",
                debug
                    .last_job_ms
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "--".to_string()),
                label_style,
                value_style,
            ));
            let engine_state_style = syntax_style(&debug.engine_state, theme, dim_style);
            rows.push(row(
                "HL State",
                debug.engine_state,
                label_style,
                engine_state_style,
            ));
        }
    }
    rows.push(row(
        "Job paused",
        format!("{}", state.job.paused),
        label_style,
        if state.job.paused {
            Style::default().fg(theme.warning)
        } else {
            dim_style
        },
    ));

    for (idx, entry) in state.visualizer.leaderboard.iter().take(3).enumerate() {
        rows.push(Row::new(vec![
            Cell::from(format!("Rule {}", idx + 1)).style(label_style),
            Cell::from(format!("{} ({:.1})", entry.rule, entry.score)).style(leaderboard_style(
                idx,
                theme,
                value_style,
            )),
        ]));
    }

    let table = Table::new(rows, [Constraint::Length(14), Constraint::Min(5)])
        .column_spacing(1)
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));

    frame.render_widget(block, area);
    frame.render_widget(table, inner);
}

fn render_games(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    syntax_status: &str,
    syntax_debug: Option<SyntaxDebugInfo>,
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

    let (ln, col) = state.line_col();
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);

    let mut rows = Vec::new();
    rows.push(row(
        "Focus",
        state.focus.title().to_string(),
        label_style,
        Style::default().fg(theme.title_focused),
    ));
    rows.push(row(
        "Mode",
        format!("{:?}", state.mode),
        label_style,
        mode_style(state.mode, theme),
    ));
    rows.push(row(
        "Dirty",
        if state.editor_buffer().is_dirty() {
            "Y"
        } else {
            "N"
        }
        .to_string(),
        label_style,
        if state.editor_buffer().is_dirty() {
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Ln/Col",
        format!("{ln}/{col}"),
        label_style,
        value_style,
    ));
    rows.push(row(
        "Bytes",
        state.editor_buffer().bytes_len().to_string(),
        label_style,
        value_style,
    ));
    rows.push(row(
        "Render(ms)",
        state.metrics.last_render_ms.to_string(),
        label_style,
        render_ms_style(state.metrics.last_render_ms, theme),
    ));
    rows.push(row(
        "Frame",
        state.metrics.frame_count.to_string(),
        label_style,
        dim_style,
    ));
    rows.push(row(
        "Workspace",
        shorten_path(&state.workspace_root, 30),
        label_style,
        value_style,
    ));
    rows.push(row(
        "Games",
        format!("{:?}", state.games.status),
        label_style,
        games_status_style(state.games.status, theme),
    ));
    rows.push(row(
        "Petri Open",
        if state.games.running { "Y" } else { "N" }.to_string(),
        label_style,
        if state.games.running {
            Style::default().fg(theme.accent)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Petri Hidden",
        if state.games.petri_hidden { "Y" } else { "N" }.to_string(),
        label_style,
        if state.games.petri_hidden {
            Style::default().fg(theme.warning)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Paused",
        if state.games.paused { "Y" } else { "N" }.to_string(),
        label_style,
        if state.games.paused {
            Style::default().fg(theme.warning)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Steps/Tick",
        state.games.steps_per_tick.to_string(),
        label_style,
        value_style,
    ));
    rows.push(row(
        "Last Run",
        state
            .games
            .last_run_path
            .as_deref()
            .map(|p| shorten_text(p, 30))
            .unwrap_or_else(|| "--".into()),
        label_style,
        if state.games.last_run_path.is_some() {
            Style::default().fg(theme.accent)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Events",
        state
            .games
            .last_event_path
            .as_deref()
            .map(|p| shorten_text(p, 30))
            .unwrap_or_else(|| "--".into()),
        label_style,
        if state.games.last_event_path.is_some() {
            Style::default().fg(theme.title)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "History",
        state
            .games
            .last_history_path
            .as_deref()
            .map(|p| shorten_text(p, 30))
            .unwrap_or_else(|| "--".into()),
        label_style,
        if state.games.last_history_path.is_some() {
            Style::default().fg(theme.title)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Syntax",
        syntax_status.to_string(),
        label_style,
        syntax_style(syntax_status, theme, dim_style),
    ));

    if let Some(info) = syntax_debug {
        rows.push(row(
            "Syn Ver",
            format!(
                "{}/{}",
                info.buffer_version,
                info.snapshot_version
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "--".into())
            ),
            label_style,
            value_style,
        ));
        rows.push(row(
            "Syn Engine",
            info.engine_state,
            label_style,
            value_style,
        ));
        rows.push(row(
            "Syn Job",
            info.last_job_ms
                .map(|ms| format!("{ms}ms"))
                .unwrap_or_else(|| "--".into()),
            label_style,
            value_style,
        ));
    }

    let table = Table::new(rows, [Constraint::Length(12), Constraint::Min(10)])
        .column_spacing(1)
        .block(block);
    frame.render_widget(table, area);
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

fn rule_name(rule_text: &str, catalog: &nit_core::RuleCatalog) -> String {
    let Ok(rule) = Rule::parse(rule_text) else {
        return "invalid".into();
    };
    catalog
        .find_by_rule(rule)
        .map(|entry| entry.name.clone())
        .unwrap_or_else(|| "--".into())
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

fn row<L: Into<String>, V: Into<String>>(
    label: L,
    value: V,
    label_style: Style,
    value_style: Style,
) -> Row<'static> {
    Row::new(vec![
        Cell::from(label.into()).style(label_style),
        Cell::from(value.into()).style(value_style),
    ])
}

fn mode_style(mode: nit_core::Mode, theme: &Theme) -> Style {
    match mode {
        nit_core::Mode::Insert => Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
        nit_core::Mode::Visual => Style::default().fg(theme.warning),
        nit_core::Mode::Normal => Style::default().fg(theme.foreground),
    }
}

fn intensity_style(intensity: GolSearchIntensity, theme: &Theme) -> Style {
    match intensity {
        GolSearchIntensity::Low => Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        GolSearchIntensity::Med => Style::default().fg(theme.title),
        GolSearchIntensity::High => Style::default().fg(theme.warning),
    }
}

fn autostop_style(policy: nit_gol::AutoStopPolicy, theme: &Theme) -> Style {
    match policy {
        nit_gol::AutoStopPolicy::Off => Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        nit_gol::AutoStopPolicy::Fixed => Style::default().fg(theme.title),
        nit_gol::AutoStopPolicy::Repeat => Style::default().fg(theme.warning),
    }
}

fn games_status_style(status: GamesStatus, theme: &Theme) -> Style {
    match status {
        GamesStatus::Idle => Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        GamesStatus::Running => Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
        GamesStatus::Paused => Style::default().fg(theme.warning),
        GamesStatus::Done => Style::default().fg(theme.title),
        GamesStatus::Error => Style::default()
            .fg(theme.error)
            .add_modifier(Modifier::BOLD),
    }
}

fn attractor_style(event: Option<&AttractorEvent>, theme: &Theme, dim_style: Style) -> Style {
    match event {
        Some(AttractorEvent::FixedPoint { .. }) => Style::default().fg(theme.title_focused),
        Some(AttractorEvent::Cycle { .. }) => Style::default().fg(theme.warning),
        None => dim_style,
    }
}

fn paused_by_style(paused: bool, by_attractor: bool, theme: &Theme, dim_style: Style) -> Style {
    if !paused {
        return dim_style;
    }
    if by_attractor {
        Style::default().fg(theme.warning)
    } else {
        Style::default().fg(theme.accent)
    }
}

fn render_ms_style(ms: u128, theme: &Theme) -> Style {
    if ms >= 60 {
        Style::default()
            .fg(theme.error)
            .add_modifier(Modifier::BOLD)
    } else if ms >= 33 {
        Style::default().fg(theme.warning)
    } else if ms >= 16 {
        Style::default().fg(theme.title)
    } else {
        Style::default().fg(theme.title_focused)
    }
}

fn tick_ms_style(ms: u64, theme: &Theme) -> Style {
    if ms <= 60 {
        Style::default().fg(theme.accent)
    } else if ms <= 120 {
        Style::default().fg(theme.title)
    } else if ms <= 300 {
        Style::default().fg(theme.foreground)
    } else {
        Style::default().fg(theme.warning)
    }
}

fn syntax_style(status: &str, theme: &Theme, dim_style: Style) -> Style {
    let upper = status.to_ascii_uppercase();
    if upper.contains("ERROR") {
        Style::default()
            .fg(theme.error)
            .add_modifier(Modifier::BOLD)
    } else if upper.contains("LAG") {
        Style::default().fg(theme.warning)
    } else if upper.contains("DISABLED") || upper.contains("OFF") {
        dim_style
    } else if upper.contains("PENDING") {
        Style::default().fg(theme.warning)
    } else if upper.contains("READY") || upper.contains("ON") || upper.contains("OK") {
        Style::default().fg(theme.title_focused)
    } else {
        Style::default().fg(theme.foreground)
    }
}

fn leaderboard_style(idx: usize, theme: &Theme, value_style: Style) -> Style {
    match idx {
        0 => Style::default().fg(theme.accent),
        1 => Style::default().fg(theme.title),
        2 => Style::default().fg(theme.border),
        _ => value_style,
    }
}
