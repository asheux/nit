use nit_core::{AppKind, AppState};
use ratatui::{
    layout::Alignment,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{block::Title, Block, Borders, Paragraph},
    Frame,
};

use crate::system_stats::{GpuSummary, SystemStats};
use crate::theme::Theme;

const MIN_INNER_HEIGHT: u16 = 3;

pub fn render(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    stats: &SystemStats,
) {
    let focus = format!("FOCUS: {}", state.focus.title());
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

    if area.height < MIN_INNER_HEIGHT {
        frame.render_widget(block, area);
        return;
    }

    let focus_span = Span::styled(
        "FOCUS",
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
    );

    let inner = block.inner(area);
    let para = Paragraph::new(Line::from(vec![focus_span]))
        .style(Style::default().bg(theme.background).fg(theme.foreground));
    frame.render_widget(block, area);
    frame.render_widget(para, inner);
}

struct BarStyles {
    label: Style,
    sep: Style,
    cpu: Style,
    gpu: Style,
    dim: Style,
    mem: Style,
    coord: Style,
}

impl BarStyles {
    fn from_theme(theme: &Theme) -> Self {
        Self {
            label: Style::default().fg(theme.title),
            sep: Style::default().fg(theme.border),
            cpu: Style::default().fg(theme.accent),
            gpu: Style::default().fg(theme.title_focused),
            dim: Style::default().fg(theme.border),
            mem: Style::default().fg(theme.warning),
            coord: Style::default().fg(theme.foreground),
        }
    }
}

fn build_metrics_line(
    state: &AppState,
    stats: &SystemStats,
    theme: &Theme,
    line: usize,
    col: usize,
) -> Line<'static> {
    let styles = BarStyles::from_theme(theme);
    let cpu = stats.cpu_percent().round().clamp(0.0, 100.0) as u8;
    let mem_used = stats.mem_used_gb();
    let mem_total = stats.mem_total_gb();
    let gpu = stats.gpu_summary();

    let mut spans = Vec::with_capacity(16);
    spans.extend([
        Span::styled("Ln ", styles.label),
        Span::styled(format!("{line}, "), styles.coord),
        Span::styled("Col ", styles.label),
        Span::styled(col.to_string(), styles.coord),
        Span::styled(" | ", styles.sep),
        Span::styled("CPU ", styles.label),
        Span::styled(format!("{cpu:02}%"), styles.cpu),
        Span::styled(" | ", styles.sep),
    ]);
    spans.extend(gpu_spans(gpu, styles.label, styles.gpu));

    if let Some(mut acc) = accelerator_spans(state, &styles) {
        spans.push(Span::styled(" | ", styles.sep));
        spans.append(&mut acc);
    }

    if mem_total > 0.0 {
        spans.push(Span::styled(" | ", styles.sep));
        spans.push(Span::styled("MEM ", styles.label));
        spans.push(Span::styled(
            format!("{mem_used:.1}/{mem_total:.1}G"),
            styles.mem,
        ));
    }

    Line::from(spans)
}

fn accelerator_spans(state: &AppState, styles: &BarStyles) -> Option<Vec<Span<'static>>> {
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

    let backend = backend_label(runtime);
    let detail = accelerator_detail(runtime);

    let mut spans = vec![
        Span::styled("ACC ", styles.label),
        Span::styled(backend, styles.gpu),
    ];
    if !detail.is_empty() {
        spans.push(Span::styled(" ", styles.dim));
        spans.push(Span::styled(detail, styles.gpu));
    }
    Some(spans)
}

fn backend_label(runtime: &nit_games::RuntimeAcceleratorStats) -> String {
    match runtime.backend {
        nit_games::RuntimeAcceleratorBackend::Metal => "MTL".into(),
        nit_games::RuntimeAcceleratorBackend::Cpu => "CPU".into(),
        nit_games::RuntimeAcceleratorBackend::None => match runtime.requested {
            nit_games::AcceleratorMode::Cpu => "CPU?".into(),
            nit_games::AcceleratorMode::Metal => "MTL?".into(),
            nit_games::AcceleratorMode::Auto => "AUTO".into(),
        },
    }
}

fn accelerator_detail(runtime: &nit_games::RuntimeAcceleratorStats) -> String {
    let mut parts: Vec<String> = Vec::new();
    let counts = [
        (runtime.metal_matches, "g"),
        (runtime.cpu_matches, "c"),
        (runtime.metal_fallbacks, "fb"),
    ];
    for (count, suffix) in counts {
        if count > 0 {
            parts.push(format!("{count}{suffix}"));
        }
    }
    if let Some(batch_part) = batch_policy_label(runtime) {
        parts.push(batch_part);
    }
    parts.join("/")
}

fn batch_policy_label(runtime: &nit_games::RuntimeAcceleratorStats) -> Option<String> {
    let batch = runtime.metal_matches_per_batch?;
    let inflight = runtime.metal_inflight_batches?;
    let mut s = format!("{batch}bx{inflight}");
    if let Some(source) = runtime.metal_policy_source_label() {
        s.push('/');
        s.push_str(source);
    }
    Some(s)
}

fn gpu_spans(gpu: GpuSummary, label_style: Style, value_style: Style) -> Vec<Span<'static>> {
    let (label, value) = match (gpu.usage_percent, gpu.mem_total_gb, gpu.name) {
        (Some(usage), Some(total), _) => ("GPU ", format!("{usage:02}%/{total:.1}G")),
        (Some(usage), None, _) => ("GPU ", format!("{usage:02}%")),
        (None, Some(total), _) => ("VRAM ", format!("{total:.1}G")),
        (None, None, Some(name)) => ("GPU ", name),
        (None, None, None) => ("GPU ", "N/A".to_string()),
    };
    vec![
        Span::styled(label, label_style),
        Span::styled(value, value_style),
    ]
}
