use nit_core::genome_report::GenomeReport;
use nit_core::seed::SeedEncoderId;
use nit_core::{AppKind, AppState, GamesStatus, PaneId, UiSelectionPane};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Gauge, Paragraph},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;

pub fn render(frame: &mut Frame, area: ratatui::layout::Rect, state: &AppState, theme: &Theme) {
    if state.app_kind == AppKind::Games {
        return render_games(frame, area, state, theme);
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

    // Determine the genome report for the active editor buffer.
    let genome_report = state
        .editor_buffer()
        .path()
        .and_then(|p| state.genome_reports.get(p));

    let title_text = match genome_report {
        Some(report) => format!(
            " CODE STRUCTURAL QUALITY \u{2014} {} {} ",
            report.tier.name(),
            tier_glyph(report.tier)
        ),
        None => " CODE STRUCTURAL QUALITY ".to_string(),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .border_type(border_type)
        .style(Style::default().bg(theme.background))
        .title(Span::styled(
            title_text,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Show loading bar while genome is being computed.
    if genome_report.is_none() && state.genome_computing {
        draw_loading_bar(frame, inner, theme);
        return;
    }

    let lines = build_lines_genome(state, genome_report, theme, inner.width as usize);
    let lines = apply_ui_selection(
        lines,
        state.ui_selection.as_ref(),
        UiSelectionPane::GateMonitor,
        theme.selection_bg,
        0,
    );
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    let scroll = state.gate_monitor_scroll.min(max_scroll);
    let para = Paragraph::new(lines)
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .scroll((scroll as u16, 0));
    frame.render_widget(para, inner);
}

pub fn build_lines(state: &AppState, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    if state.app_kind == AppKind::Games {
        build_lines_games(state, theme, width)
    } else {
        let report = state
            .editor_buffer()
            .path()
            .and_then(|p| state.genome_reports.get(p));
        build_lines_genome(state, report, theme, width)
    }
}

// ---------------------------------------------------------------------------
// Genome quality dashboard
// ---------------------------------------------------------------------------

fn build_lines_genome(
    _state: &AppState,
    report: Option<&GenomeReport>,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let Some(report) = report else {
        return vec![
            Line::from(""),
            Line::from(Span::styled(
                "No file open",
                Style::default().fg(theme.border),
            )),
        ];
    };

    let mut lines: Vec<Line<'static>> = Vec::new();

    // ── Tier display ──
    let tier_text = format!("TIER {}  {}", report.tier.numeral(), report.tier.name());
    lines.push(Line::from(Span::styled(
        tier_text,
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(build_tier_bar(report.tier, width, theme));
    lines.push(Line::from(""));

    // ── Consistency gauge ──
    lines.push(build_gauge_line(
        "Consistency",
        report.cross_encoder_consistency,
        width,
        theme,
    ));
    lines.push(Line::from(""));

    // ── AST Encoders (determine tier) ──
    lines.push(Line::from(vec![
        Span::styled("AST Encoders ", label_style),
        Span::styled("─".repeat(width.saturating_sub(14)), dim_style),
    ]));
    let ast_encoders = [
        SeedEncoderId::TokenSpectrum,
        SeedEncoderId::AstStructure,
        SeedEncoderId::ComplexityField,
    ];
    for &enc_id in &ast_encoders {
        if let Some(score) = report.encoder_scores.iter().find(|s| s.encoder == enc_id) {
            lines.push(build_encoder_line(score, width, theme, true));
        }
    }
    lines.push(Line::from(""));

    // ── Byte Encoders ──
    lines.push(Line::from(vec![
        Span::styled("Byte Encoders ", label_style),
        Span::styled("─".repeat(width.saturating_sub(15)), dim_style),
    ]));
    let byte_encoders = [
        SeedEncoderId::AsciiBytes,
        SeedEncoderId::Lifehash16,
        SeedEncoderId::HilbertBits,
        SeedEncoderId::Structural,
    ];
    for &enc_id in &byte_encoders {
        if let Some(score) = report.encoder_scores.iter().find(|s| s.encoder == enc_id) {
            lines.push(build_encoder_line(score, width, theme, false));
        }
    }
    lines.push(Line::from(""));

    // ── Density breakdown ──
    lines.push(Line::from(vec![
        Span::styled("Density ", label_style),
        Span::styled("─".repeat(width.saturating_sub(9)), dim_style),
    ]));
    for &enc_id in &ast_encoders {
        if let Some(score) = report.encoder_scores.iter().find(|s| s.encoder == enc_id) {
            lines.push(build_density_line(score, width, theme));
        }
    }
    lines.push(Line::from(""));

    // ── Peak & cycle summary ──
    lines.push(Line::from(vec![
        Span::styled("Simulation ", label_style),
        Span::styled("─".repeat(width.saturating_sub(12)), dim_style),
    ]));
    for score in &report.encoder_scores {
        lines.push(build_sim_detail_line(score, width, theme));
    }
    lines.push(Line::from(""));

    // ── Issues ──
    if !report.recommendations.is_empty() {
        let critical = report
            .recommendations
            .iter()
            .filter(|r| matches!(r.severity, nit_core::RecommendationSeverity::Critical))
            .count();
        let warning = report
            .recommendations
            .iter()
            .filter(|r| matches!(r.severity, nit_core::RecommendationSeverity::Warning))
            .count();
        let info = report
            .recommendations
            .iter()
            .filter(|r| matches!(r.severity, nit_core::RecommendationSeverity::Info))
            .count();

        let mut summary_spans: Vec<Span<'static>> = vec![Span::styled("Issues ", label_style)];
        if critical > 0 {
            summary_spans.push(Span::styled(
                format!("{critical} critical"),
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD),
            ));
            if warning > 0 || info > 0 {
                summary_spans.push(Span::styled("  ", dim_style));
            }
        }
        if warning > 0 {
            summary_spans.push(Span::styled(
                format!("{warning} warn"),
                Style::default().fg(theme.title_focused),
            ));
            if info > 0 {
                summary_spans.push(Span::styled("  ", dim_style));
            }
        }
        if info > 0 {
            summary_spans.push(Span::styled(format!("{info} info"), dim_style));
        }
        // Fill remainder with separator
        let used: usize = summary_spans.iter().map(|s| s.content.len()).sum();
        if width > used + 1 {
            summary_spans.push(Span::styled(
                format!(" {}", "─".repeat(width.saturating_sub(used + 2))),
                dim_style,
            ));
        }
        lines.push(Line::from(summary_spans));

        // Show critical items in full.
        for rec in &report.recommendations {
            if !matches!(rec.severity, nit_core::RecommendationSeverity::Critical) {
                continue;
            }
            let loc = rec
                .location
                .as_deref()
                .map(|l| format!(" ({l})"))
                .unwrap_or_default();
            let msg = shorten_text(
                &format!("{}{loc}", metric_display_name(&rec.metric)),
                width.saturating_sub(4),
            );
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(theme.foreground)),
                Span::styled(
                    msg,
                    Style::default()
                        .fg(theme.foreground)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        // Warnings: show each individually with location.
        for rec in &report.recommendations {
            if !matches!(rec.severity, nit_core::RecommendationSeverity::Warning) {
                continue;
            }
            let loc = rec
                .location
                .as_deref()
                .map(|l| format!(" ({l})"))
                .unwrap_or_default();
            let label = metric_display_name(&rec.metric);
            let msg = shorten_text(&format!("{label}{loc}"), width.saturating_sub(4));
            lines.push(Line::from(Span::styled(
                format!("  {msg}"),
                Style::default().fg(theme.title_focused),
            )));
        }

        // Info: show each individually with location.
        for rec in &report.recommendations {
            if !matches!(rec.severity, nit_core::RecommendationSeverity::Info) {
                continue;
            }
            let loc = rec
                .location
                .as_deref()
                .map(|l| format!(" ({l})"))
                .unwrap_or_default();
            let label = metric_display_name(&rec.metric);
            let msg = shorten_text(&format!("{label}{loc}"), width.saturating_sub(4));
            lines.push(Line::from(Span::styled(format!("  {msg}"), dim_style)));
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("No structural issues ", Style::default().fg(theme.accent)),
            Span::styled("─".repeat(width.saturating_sub(22)), dim_style),
        ]));
    }

    lines
}

/// Build a horizontal bar representing the tier level (I-V), full width.
fn build_tier_bar(tier: nit_core::GenomeTier, width: usize, theme: &Theme) -> Line<'static> {
    if width < 5 {
        return Line::from("");
    }
    let tier_val = tier as u32; // 0..4
    let filled = ((tier_val + 1) as usize * width) / 5;
    let empty = width.saturating_sub(filled);
    let bar_color = tier_color(tier, theme);

    Line::from(vec![
        Span::styled("━".repeat(filled), Style::default().fg(bar_color)),
        Span::styled(
            "╌".repeat(empty),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ),
    ])
}

/// Build a gauge line filling the full width: "Label  ▓▓▓▓░░░░░░░░  0.23"
fn build_gauge_line(label: &str, value: f32, width: usize, theme: &Theme) -> Line<'static> {
    let label_w = 14;
    let num_str = format!("{value:.2}");
    let num_w = num_str.len() + 1;
    let bar_w = width.saturating_sub(label_w + num_w + 1);

    let clamped = value.clamp(0.0, 1.0);
    let filled = (clamped * bar_w as f32).round() as usize;
    let empty = bar_w.saturating_sub(filled);

    let bar_color = if clamped >= 0.6 {
        theme.accent
    } else if clamped >= 0.3 {
        theme.title_focused
    } else {
        theme.title
    };

    Line::from(vec![
        Span::styled(
            pad_to_width(label, label_w),
            Style::default().fg(theme.title).add_modifier(Modifier::DIM),
        ),
        Span::styled("▓".repeat(filled), Style::default().fg(bar_color)),
        Span::styled(
            "░".repeat(empty),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(format!(" {num_str}"), Style::default().fg(theme.foreground)),
    ])
}

/// Build an encoder line with a bar filling all space between name and stats.
fn build_encoder_line(
    score: &nit_core::EncoderScore,
    width: usize,
    theme: &Theme,
    is_ast: bool,
) -> Line<'static> {
    let name = encoder_short_name(score.encoder);
    let name_w = 12;
    let gen = score.generations_survived;
    let gen_str = format!("{gen}");
    let stats_str = format!(
        " density={:.2} components={}",
        score.density, score.components
    );
    let fixed_w = name_w + gen_str.len() + stats_str.len() + 2;
    let bar_w = width.saturating_sub(fixed_w);

    let ratio = (gen as f32 / 3000.0).clamp(0.0, 1.0);
    let filled = (ratio * bar_w as f32).round() as usize;
    let empty = bar_w.saturating_sub(filled);
    let bar_color = gen_color(gen, theme);

    let name_style = if is_ast {
        Style::default().fg(theme.title_focused)
    } else {
        Style::default().fg(theme.title).add_modifier(Modifier::DIM)
    };

    Line::from(vec![
        Span::styled(pad_to_width(name, name_w), name_style),
        Span::styled("▓".repeat(filled), Style::default().fg(bar_color)),
        Span::styled(
            "░".repeat(empty),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            format!(" {gen_str}"),
            Style::default().fg(bar_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(stats_str, Style::default().fg(theme.border)),
    ])
}

/// Build a density bar for an AST encoder, full width with threshold marker.
fn build_density_line(
    score: &nit_core::EncoderScore,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let name = encoder_short_name(score.encoder);
    let name_w = 12;
    let val_str = format!("{:.2}", score.density);
    let fixed_w = name_w + val_str.len() + 2;
    let bar_w = width.saturating_sub(fixed_w);

    let clamped = score.density.clamp(0.0, 1.0);
    let filled = (clamped * bar_w as f32).round() as usize;
    let empty = bar_w.saturating_sub(filled);

    let over_threshold = score.density > 0.45;
    let bar_color = if over_threshold {
        theme.foreground
    } else if score.density > 0.35 {
        theme.title
    } else {
        theme.title_focused
    };

    let mut spans = vec![
        Span::styled(
            pad_to_width(name, name_w),
            Style::default().fg(theme.title).add_modifier(Modifier::DIM),
        ),
        Span::styled("▓".repeat(filled), Style::default().fg(bar_color)),
        Span::styled(
            "░".repeat(empty),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            format!(" {val_str}"),
            Style::default().fg(if over_threshold {
                theme.foreground
            } else {
                theme.title
            }),
        ),
    ];
    if over_threshold {
        spans.push(Span::styled(
            " !",
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

/// Build a compact simulation detail line for one encoder.
fn build_sim_detail_line(
    score: &nit_core::EncoderScore,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let name = encoder_short_name(score.encoder);
    let name_w = 12;
    let gen_color = gen_color(score.generations_survived, theme);
    let dim = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);

    let cycle_str = match score.cycle_period {
        Some(p) => format!("period={p}"),
        None => "none".into(),
    };
    let detail = format!(
        "generations={:<5} peak={:<5} cycle={}",
        score.generations_survived, score.peak_population, cycle_str
    );
    let used = name_w + detail.len();
    let pad = width.saturating_sub(used);

    Line::from(vec![
        Span::styled(
            pad_to_width(name, name_w),
            Style::default().fg(theme.title).add_modifier(Modifier::DIM),
        ),
        Span::styled(
            format!("generations={:<5}", score.generations_survived),
            Style::default().fg(gen_color),
        ),
        Span::styled(
            format!(" peak={:<5}", score.peak_population),
            Style::default().fg(theme.foreground),
        ),
        Span::styled(format!(" cycle={cycle_str}"), dim),
        Span::styled(" ".repeat(pad.saturating_sub(2)), dim),
    ])
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn tier_glyph(tier: nit_core::GenomeTier) -> &'static str {
    match tier {
        nit_core::GenomeTier::StillLife => ".",
        nit_core::GenomeTier::Oscillator => "~",
        nit_core::GenomeTier::Spaceship => ">",
        nit_core::GenomeTier::Methuselah => "*",
        nit_core::GenomeTier::Replicator => "!",
    }
}

fn tier_color(tier: nit_core::GenomeTier, theme: &Theme) -> ratatui::style::Color {
    match tier {
        nit_core::GenomeTier::StillLife => theme.border,
        nit_core::GenomeTier::Oscillator => theme.title,
        nit_core::GenomeTier::Spaceship => theme.title_focused,
        nit_core::GenomeTier::Methuselah => theme.accent,
        nit_core::GenomeTier::Replicator => theme.accent,
    }
}

/// Animated loading bar centered vertically in the given area.
/// Identical to the visualizer pane's loading bar.
fn draw_loading_bar(frame: &mut Frame, area: ratatui::layout::Rect, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let bar_w = area.width / 3;
    let x = area.x + (area.width.saturating_sub(bar_w)) / 2;
    let y = area.y.saturating_add(area.height / 2);
    let bar_area = ratatui::layout::Rect {
        x,
        y,
        width: bar_w,
        height: 1,
    };
    let ratio = loading_ratio();
    let gauge = Gauge::default()
        .block(Block::default().style(Style::default().bg(theme.background)))
        .gauge_style(
            Style::default()
                .fg(theme.title_focused)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD),
        )
        .ratio(ratio)
        .label(Span::styled(
            "Genome loading",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::DIM),
        ));
    frame.render_widget(gauge, bar_area);
}

/// Triangular wave oscillating 0..1..0 over 1600ms, driven by system clock.
fn loading_ratio() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let millis = now.as_millis() as f64;
    let period = 1600.0;
    let phase = (millis % period) / period;
    let tri = if phase <= 0.5 {
        phase * 2.0
    } else {
        (1.0 - phase) * 2.0
    };
    tri.clamp(0.0, 1.0)
}

fn gen_color(gen: u32, theme: &Theme) -> ratatui::style::Color {
    match gen {
        0..=50 => theme.border,
        51..=200 => theme.title,
        201..=500 => theme.title_focused,
        501..=2000 => theme.accent,
        _ => theme.accent,
    }
}

fn encoder_short_name(id: SeedEncoderId) -> &'static str {
    match id {
        SeedEncoderId::AsciiBytes => "AsciiBytes",
        SeedEncoderId::Lifehash16 => "Lifehash16",
        SeedEncoderId::HilbertBits => "HilbertBits",
        SeedEncoderId::Structural => "Structural",
        SeedEncoderId::TokenSpectrum => "TokenSpec",
        SeedEncoderId::AstStructure => "AstStruct",
        SeedEncoderId::ComplexityField => "Complexity",
    }
}

fn metric_display_name(metric: &str) -> &str {
    match metric {
        "cyclomatic_complexity" => "high cyclomatic complexity",
        "nesting_depth" => "deep nesting",
        "identifier_uniqueness" => "identifier reuse",
        "token_entropy" => "low token diversity",
        "density" => "high density",
        "components" => "low modularity",
        other => other,
    }
}

fn shorten_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }
    if max_len < 4 {
        return text.chars().take(max_len).collect();
    }
    let truncated: String = text.chars().take(max_len - 1).collect();
    format!("{truncated}\u{2026}")
}

fn pad_to_width(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(width).collect();
    let count = out.chars().count();
    if count < width {
        out.push_str(&" ".repeat(width - count));
    }
    out
}

fn right_align(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let trimmed: String = text.chars().take(width).collect();
    let count = trimmed.chars().count();
    if count < width {
        format!("{}{}", " ".repeat(width - count), trimmed)
    } else {
        trimmed
    }
}

// ---------------------------------------------------------------------------
// Games mode (kept as-is)
// ---------------------------------------------------------------------------

fn render_games(frame: &mut Frame, area: ratatui::layout::Rect, state: &AppState, theme: &Theme) {
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

    let lines = build_lines_games(state, theme, inner.width as usize);
    let lines = apply_ui_selection(
        lines,
        state.ui_selection.as_ref(),
        UiSelectionPane::GateMonitor,
        theme.selection_bg,
        0,
    );
    let para =
        Paragraph::new(lines).style(Style::default().bg(theme.background).fg(theme.foreground));
    frame.render_widget(block, area);
    frame.render_widget(para, inner);
}

fn build_lines_games(state: &AppState, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let (ln, col) = state.line_col();
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);

    let syntax_status = state.syntax_status.as_str();
    let mut rows: Vec<GateLine> = Vec::new();
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
            Style::default().fg(theme.title_focused)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Petri Hide",
        if state.games.petri_hidden { "Y" } else { "N" }.to_string(),
        label_style,
        if state.games.petri_hidden {
            Style::default().fg(theme.warning)
        } else {
            dim_style
        },
    ));
    rows.push(row(
        "Petri Pause",
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
            .unwrap_or("--")
            .to_string(),
        label_style,
        dim_style,
    ));
    rows.push(row(
        "Syntax",
        syntax_status.to_string(),
        label_style,
        syntax_style(syntax_status, theme, dim_style),
    ));

    format_rows(rows, 12, width)
}

// ---------------------------------------------------------------------------
// Shared formatting helpers (used by Games mode)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct GateLine {
    label: String,
    value: String,
    label_style: Style,
    value_style: Style,
}

fn row<L: Into<String>, V: Into<String>>(
    label: L,
    value: V,
    label_style: Style,
    value_style: Style,
) -> GateLine {
    GateLine {
        label: label.into(),
        value: value.into(),
        label_style,
        value_style,
    }
}

fn format_rows(rows: Vec<GateLine>, label_width: usize, max_width: usize) -> Vec<Line<'static>> {
    if max_width == 0 {
        return Vec::new();
    }
    let label_width = label_width.min(max_width);
    let value_width = max_width.saturating_sub(label_width + 1);
    rows.into_iter()
        .map(|row| {
            let label = pad_to_width(&row.label, label_width);
            if value_width == 0 {
                Line::from(Span::styled(label, row.label_style))
            } else {
                let value = right_align(&row.value, value_width);
                Line::from(vec![
                    Span::styled(label, row.label_style),
                    Span::raw(" "),
                    Span::styled(value, row.value_style),
                ])
            }
        })
        .collect()
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
