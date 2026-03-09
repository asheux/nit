use nit_core::{AppKind, AppState, PaneId};
use ratatui::{
    layout::Alignment,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{block::Title, Block, Borders, Paragraph},
    Frame,
};

use crate::system_stats::{GpuSummary, SystemStats};
use crate::theme::Theme;

pub fn render(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    stats: &SystemStats,
) {
    let focus = format!("FOCUS: {}", focus_name(state.focus));
    let (line, col) = state.line_col();
    let metrics = build_metrics_line(state, stats, theme, line, col);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background));

    let title_style = Style::default()
        .fg(theme.foreground)
        .add_modifier(Modifier::DIM);
    let block = block
        .title(Title::from(Span::styled(focus, title_style)).alignment(Alignment::Left))
        .title(Title::from(metrics).alignment(Alignment::Right));

    if area.height <= 2 {
        frame.render_widget(block, area);
        return;
    }

    let spans = vec![Span::styled(
        "FOCUS",
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
    )];

    let inner = block.inner(area);
    let para = Paragraph::new(Line::from(spans))
        .style(Style::default().bg(theme.background).fg(theme.foreground));
    frame.render_widget(block, area);
    frame.render_widget(para, inner);
}

fn focus_name(pane: PaneId) -> &'static str {
    pane.title()
}

fn build_metrics_line(
    state: &AppState,
    stats: &SystemStats,
    theme: &Theme,
    line: usize,
    col: usize,
) -> Line<'static> {
    let cpu = stats.cpu_percent().round().clamp(0.0, 100.0) as u8;
    let mem_used = stats.mem_used_gb();
    let mem_total = stats.mem_total_gb();
    let gpu = stats.gpu_summary();

    let label_style = Style::default().fg(theme.title);
    let sep_style = Style::default().fg(theme.border);
    let cpu_style = Style::default().fg(theme.accent);
    let gpu_style = Style::default().fg(theme.title_focused);
    let dim_style = Style::default().fg(theme.border);
    let mem_style = Style::default().fg(theme.warning);
    let coord_style = Style::default().fg(theme.foreground);

    let mut spans = Vec::new();
    spans.push(Span::styled("Ln ", label_style));
    spans.push(Span::styled(format!("{line}, "), coord_style));
    spans.push(Span::styled("Col ", label_style));
    spans.push(Span::styled(format!("{col}"), coord_style));
    spans.push(Span::styled(" | ", sep_style));

    spans.push(Span::styled("CPU ", label_style));
    spans.push(Span::styled(format!("{cpu:02}%"), cpu_style));

    spans.push(Span::styled(" | ", sep_style));
    spans.extend(gpu_spans(gpu, label_style, gpu_style));

    if let Some(mut acc_spans) = accelerator_spans(state, label_style, gpu_style, dim_style) {
        spans.push(Span::styled(" | ", sep_style));
        spans.append(&mut acc_spans);
    }

    if mem_total > 0.0 {
        spans.push(Span::styled(" | ", sep_style));
        spans.push(Span::styled("MEM ", label_style));
        spans.push(Span::styled(
            format!("{mem_used:.1}/{mem_total:.1}G"),
            mem_style,
        ));
    }

    Line::from(spans)
}

fn accelerator_spans(
    state: &AppState,
    label_style: Style,
    value_style: Style,
    dim_style: Style,
) -> Option<Vec<Span<'static>>> {
    if state.app_kind != AppKind::Games {
        return None;
    }
    let runtime = &state.games.runtime;
    let show = state.games.running
        || state.games.last_run.is_some()
        || runtime.metal_matches > 0
        || runtime.cpu_matches > 0
        || runtime.metal_fallbacks > 0;
    if !show {
        return None;
    }

    let backend = match runtime.backend {
        nit_games::RuntimeAcceleratorBackend::Metal => "MTL".to_string(),
        nit_games::RuntimeAcceleratorBackend::Cpu => "CPU".to_string(),
        nit_games::RuntimeAcceleratorBackend::None => match runtime.requested {
            nit_games::AcceleratorMode::Cpu => "CPU?".to_string(),
            nit_games::AcceleratorMode::Metal => "MTL?".to_string(),
            nit_games::AcceleratorMode::Auto => "AUTO".to_string(),
        },
    };
    let mut detail = String::new();
    if runtime.metal_matches > 0 {
        detail.push_str(&format!("{}g", runtime.metal_matches));
    }
    if runtime.cpu_matches > 0 {
        if !detail.is_empty() {
            detail.push('/');
        }
        detail.push_str(&format!("{}c", runtime.cpu_matches));
    }
    if runtime.metal_fallbacks > 0 {
        if !detail.is_empty() {
            detail.push('/');
        }
        detail.push_str(&format!("{}fb", runtime.metal_fallbacks));
    }
    if let (Some(batch), Some(inflight)) = (
        runtime.metal_matches_per_batch,
        runtime.metal_inflight_batches,
    ) {
        if !detail.is_empty() {
            detail.push('/');
        }
        detail.push_str(&format!("{}bx{}", batch, inflight));
        if let Some(source) = runtime.metal_policy_source_label() {
            detail.push('/');
            detail.push_str(source);
        }
    }

    let mut spans = vec![
        Span::styled("ACC ", label_style),
        Span::styled(backend, value_style),
    ];
    if !detail.is_empty() {
        spans.push(Span::styled(" ", dim_style));
        spans.push(Span::styled(detail, value_style));
    }
    Some(spans)
}

fn gpu_spans(gpu: GpuSummary, label_style: Style, value_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let (label, value) = match (gpu.usage_percent, gpu.mem_total_gb, gpu.name) {
        (Some(usage), Some(total), _) => ("GPU ", format!("{usage:02}%/{total:.1}G")),
        (Some(usage), None, _) => ("GPU ", format!("{usage:02}%")),
        (None, Some(total), _) => ("VRAM ", format!("{total:.1}G")),
        (None, None, Some(name)) => ("GPU ", name),
        (None, None, None) => ("GPU ", "N/A".to_string()),
    };
    spans.push(Span::styled(label, label_style));
    spans.push(Span::styled(value, value_style));
    spans
}
