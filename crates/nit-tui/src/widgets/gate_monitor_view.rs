use nit_core::genome_report::GenomeReport;
use nit_core::seed::SeedEncoderId;
use nit_core::{
    Action, AppKind, AppState, GamesStatus, GateMonitorSubView, PaneId, UiSelectionPane,
};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;

// Title button column ranges (relative to rect start + 1 for border).
// " CODE STRUCTURAL QUALITY [NxN] " then " STATS " then " FILESCORES "
const BTN_STATS_LABEL: &str = " STATS ";
const BTN_FILESCORES_LABEL: &str = " FILESCORES ";

/// Returns an action if the click column hits a title button.
pub fn title_button_hit(col_in_rect: u16, title_prefix_len: u16) -> Option<Action> {
    let col = col_in_rect.saturating_sub(1); // border offset
    let stats_start = title_prefix_len + 1; // space separator
    let stats_end = stats_start + BTN_STATS_LABEL.len() as u16;
    let fs_start = stats_end + 1;
    let fs_end = fs_start + BTN_FILESCORES_LABEL.len() as u16;
    if (stats_start..stats_end).contains(&col) || (fs_start..fs_end).contains(&col) {
        Some(Action::GateMonitorToggleSubView)
    } else {
        None
    }
}

pub fn render(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &mut AppState,
    theme: &Theme,
) {
    if state.app_kind == AppKind::Games {
        // Games variant has no scroll state to cache.
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

    let title_prefix = match genome_report {
        Some(report) => format!(
            " CODE STRUCTURAL QUALITY [{}x{}] ",
            report.grid_size, report.grid_size,
        ),
        None => " CODE STRUCTURAL QUALITY ".to_string(),
    };

    let title_style = Style::default()
        .fg(title_color)
        .add_modifier(Modifier::BOLD);
    let btn_active = Style::default()
        .fg(theme.background)
        .bg(title_color)
        .add_modifier(Modifier::BOLD);
    let btn_inactive = Style::default().fg(title_color).add_modifier(Modifier::DIM);
    let sep_style = Style::default().fg(title_color);

    let is_stats = state.gate_monitor_sub_view == GateMonitorSubView::Stats;
    let stats_style = if is_stats { btn_active } else { btn_inactive };
    let fs_style = if is_stats { btn_inactive } else { btn_active };

    let title = Line::from(vec![
        Span::styled(title_prefix, title_style),
        Span::styled(" ", sep_style),
        Span::styled(BTN_STATS_LABEL, stats_style),
        Span::styled(" ", sep_style),
        Span::styled(BTN_FILESCORES_LABEL, fs_style),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .border_type(border_type)
        .style(Style::default().bg(theme.background))
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Show loading bar while genome is being computed.
    if genome_report.is_none() && state.genome_computing {
        draw_loading_bar(frame, inner, theme);
        return;
    }

    // Show centered placeholder when no file is open (Stats view only).
    if genome_report.is_none() && state.gate_monitor_sub_view == GateMonitorSubView::Stats {
        draw_no_file_placeholder(frame, inner, theme);
        return;
    }

    let lines = match state.gate_monitor_sub_view {
        GateMonitorSubView::Stats => {
            build_lines_genome(state, genome_report, theme, inner.width as usize)
        }
        GateMonitorSubView::FileScores => {
            build_lines_filescores(state, theme, inner.width as usize)
        }
    };
    let lines = apply_ui_selection(
        lines,
        state.ui_selection.as_ref(),
        UiSelectionPane::GateMonitor,
        theme.selection_bg,
        0,
    );
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    let scroll = state.gate_monitor_scroll.min(max_scroll);
    // Cache max_scroll + clamp stored offset so scroll handlers can skip
    // rebuilding the genome report on every wheel tick.
    state.gate_monitor_last_max_scroll = max_scroll;
    state.gate_monitor_scroll = scroll;
    let para = Paragraph::new(lines)
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .scroll((scroll as u16, 0));
    frame.render_widget(para, inner);
}

pub fn build_lines(state: &AppState, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    if state.app_kind == AppKind::Games {
        build_lines_games(state, theme, width)
    } else {
        match state.gate_monitor_sub_view {
            GateMonitorSubView::Stats => {
                let report = state
                    .editor_buffer()
                    .path()
                    .and_then(|p| state.genome_reports.get(p));
                build_lines_genome(state, report, theme, width)
            }
            GateMonitorSubView::FileScores => build_lines_filescores(state, theme, width),
        }
    }
}

// ---------------------------------------------------------------------------
// Genome quality dashboard
// ---------------------------------------------------------------------------

fn build_lines_genome(
    state: &AppState,
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

    // ── Tier display with quality change indicator ──
    let tier_text = format!("TIER {}  {}", report.tier.numeral(), report.tier.name());
    let mut tier_spans = vec![Span::styled(
        tier_text,
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )];
    // Derive quality delta directly from current report vs baseline.
    // This is always correct regardless of async eval timing.
    let display_delta = if let Some(file_path) = state.editor_buffer().path() {
        if let Some(base) = state.genome_baselines.get(file_path) {
            if report.tier > base.tier {
                1
            } else if report.tier < base.tier {
                -1
            } else {
                let gen_base: i32 = base
                    .encoder_scores
                    .iter()
                    .map(|s| s.generations_survived as i32)
                    .sum();
                let gen_now: i32 = report
                    .encoder_scores
                    .iter()
                    .map(|s| s.generations_survived as i32)
                    .sum();
                gen_now.cmp(&gen_base) as i32
            }
        } else {
            state.genome_quality_delta
        }
    } else {
        state.genome_quality_delta
    };
    match display_delta {
        d if d > 0 => {
            tier_spans.push(Span::styled(
                " \u{2014} Quality Improved",
                Style::default().fg(theme.success),
            ));
        }
        d if d < 0 => {
            tier_spans.push(Span::styled(
                " \u{2014} Quality Degraded",
                Style::default().fg(theme.error),
            ));
        }
        0 => {
            tier_spans.push(Span::styled(
                " \u{2014} Quality Unchanged",
                Style::default().fg(theme.warning),
            ));
        }
        _ => {}
    }
    lines.push(Line::from(tier_spans));
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

    // ── Quality Encoders (3 AST + 1 hybrid) ──
    lines.push(Line::from(vec![
        Span::styled("Encoders ", label_style),
        Span::styled("─".repeat(width.saturating_sub(10)), dim_style),
    ]));
    let quality_encoders = [
        SeedEncoderId::TokenSpectrum,
        SeedEncoderId::AstStructure,
        SeedEncoderId::ComplexityField,
        SeedEncoderId::Structural,
    ];
    for &enc_id in &quality_encoders {
        // AST encoders determine tier; Structural is hybrid.
        let is_ast = matches!(
            enc_id,
            SeedEncoderId::TokenSpectrum
                | SeedEncoderId::AstStructure
                | SeedEncoderId::ComplexityField
        );
        if let Some(score) = report.encoder_scores.iter().find(|s| s.encoder == enc_id) {
            lines.push(build_encoder_line(score, width, theme, is_ast));
        }
    }
    lines.push(Line::from(""));

    // ── Density breakdown ──
    lines.push(Line::from(vec![
        Span::styled("Density ", label_style),
        Span::styled("─".repeat(width.saturating_sub(9)), dim_style),
    ]));
    for &enc_id in &quality_encoders {
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

    // Shadow evaluator section: real-time per-file quality during agent turns.
    if !state.genome_turn_active.is_empty() && !state.genome_shadow_evals.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                "SHADOW EVALUATOR ",
                Style::default()
                    .fg(theme.title)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("─".repeat(width.saturating_sub(18)), dim_style),
        ]));
        let mut sorted: Vec<_> = state.genome_shadow_evals.iter().collect();
        sorted.sort_by_key(|(p, _)| (*p).clone());
        for (path, eval) in &sorted {
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            let delta_color = match eval.delta_label {
                "improved" => theme.accent,
                "degraded" => theme.error,
                "new" => theme.title,
                _ => theme.border,
            };
            let new_tag = if eval.is_new_file { " [NEW]" } else { "" };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {file_name}{new_tag} "),
                    Style::default().fg(theme.foreground),
                ),
                Span::styled(
                    format!("{} ", eval.delta_label),
                    Style::default().fg(delta_color),
                ),
                Span::styled(
                    format!("tier {} c={:.2}", eval.tier.numeral(), eval.consistency,),
                    dim_style,
                ),
            ]));
        }
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
    let growth_str = score.growth_class.label();
    let detail = format!(
        "generations={:<5} peak={:<5} growth={:<10} cycle={}",
        score.generations_survived, score.peak_population, growth_str, cycle_str
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
        Span::styled(
            format!(" {growth_str}"),
            Style::default().fg(match score.growth_class {
                nit_core::GrowthClass::Expanding => theme.accent,
                nit_core::GrowthClass::Stable => theme.foreground,
                nit_core::GrowthClass::Collapsing => theme.error,
                nit_core::GrowthClass::Extinct => theme.error,
            }),
        ),
        Span::styled(format!(" cycle={cycle_str}"), dim),
        Span::styled(" ".repeat(pad.saturating_sub(2)), dim),
    ])
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn tier_color(tier: nit_core::GenomeTier, theme: &Theme) -> ratatui::style::Color {
    match tier {
        nit_core::GenomeTier::StillLife => theme.border,
        nit_core::GenomeTier::Oscillator => theme.title,
        nit_core::GenomeTier::Spaceship => theme.title_focused,
        nit_core::GenomeTier::Methuselah => theme.accent,
        nit_core::GenomeTier::Replicator => theme.accent,
    }
}

/// Animated indeterminate loading bar centered vertically in the given area.
/// Centered placeholder bar matching the visualizer's "Open file" style.
fn draw_no_file_placeholder(frame: &mut Frame, area: ratatui::layout::Rect, theme: &Theme) {
    if area.width < 4 || area.height < 3 {
        return;
    }
    let msg = " Open file in editor to view code genome ";
    let msg_len = msg.len() as u16;
    let bar_w = msg_len;
    let bar_h = 3u16;
    let bar_x = area.x + area.width.saturating_sub(bar_w) / 2;
    let bar_y = area.y + area.height.saturating_sub(bar_h) / 2;

    let cyan = ratatui::style::Color::Rgb(0, 215, 215);
    let bar_style = Style::default().bg(cyan);
    let text_style = Style::default()
        .fg(theme.background)
        .bg(cyan)
        .add_modifier(Modifier::BOLD);

    let buf = frame.buffer_mut();
    for row in 0..bar_h {
        for dx in 0..bar_w {
            let cell = buf.get_mut(bar_x + dx, bar_y + row);
            cell.set_char(' ');
            cell.set_style(bar_style);
        }
    }
    let text_y = bar_y + 1;
    for (i, ch) in msg.chars().enumerate() {
        let cx = bar_x + i as u16;
        if cx < bar_x + bar_w {
            let cell = buf.get_mut(cx, text_y);
            cell.set_char(ch);
            cell.set_style(text_style);
        }
    }
}

fn draw_loading_bar(frame: &mut Frame, area: ratatui::layout::Rect, theme: &Theme) {
    if area.width < 6 || area.height < 2 {
        return;
    }
    let bar_w = area.width / 3;
    let x = area.x + (area.width.saturating_sub(bar_w)) / 2;
    let y = area.y.saturating_add(area.height / 2);

    let label = "Genome loading";
    let label_y = y.saturating_sub(1);
    if label_y >= area.y {
        let label_x = x + bar_w.saturating_sub(label.len() as u16) / 2;
        let label_style = Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::DIM);
        let buf = frame.buffer_mut();
        for (i, ch) in label.chars().enumerate() {
            let cx = label_x + i as u16;
            if cx < x + bar_w {
                buf.get_mut(cx, label_y).set_char(ch).set_style(label_style);
            }
        }
    }

    let ratio = loading_ratio();
    let seg_w = (bar_w / 5).max(2);
    let travel = bar_w.saturating_sub(seg_w);
    let seg_x = x + (travel as f64 * ratio) as u16;

    let track_style = Style::default()
        .fg(theme.title_focused)
        .add_modifier(Modifier::DIM);
    let seg_style = Style::default()
        .fg(theme.title_focused)
        .add_modifier(Modifier::BOLD);
    let buf = frame.buffer_mut();
    for dx in 0..bar_w {
        let cx = x + dx;
        if cx >= seg_x && cx < seg_x + seg_w {
            buf.get_mut(cx, y).set_char('━').set_style(seg_style);
        } else {
            buf.get_mut(cx, y).set_char('─').set_style(track_style);
        }
    }
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

// ---------------------------------------------------------------------------
// FILESCORES sub-view: real-time file quality table
// ---------------------------------------------------------------------------

fn build_lines_filescores(state: &AppState, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let header_style = Style::default()
        .fg(theme.title)
        .add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Helper: check if a path contains a gitignored directory component.
    let is_gitignored = |path: &std::path::Path| -> bool {
        path.components().any(|c| {
            if let std::path::Component::Normal(s) = c {
                let s = s.to_string_lossy();
                state.gitignored_dirs.iter().any(|g| g == s.as_ref())
            } else {
                false
            }
        })
    };

    // Collect all files with scores from three sources, deduplicating by path.
    // Priority: shadow evals (most current) > turn-modified reports > all genome reports.
    let mut seen = std::collections::HashSet::new();
    let mut file_rows: Vec<FileScoreRow> = Vec::new();

    // Helper: compute delta from baselines. Returns "—" when no baseline exists
    // (file was never evaluated before this session).
    let compute_delta = |path: &std::path::Path, tier: nit_core::GenomeTier| -> &'static str {
        match state.genome_baselines.get(path) {
            Some(base) => {
                if tier > base.tier {
                    "improved"
                } else if tier < base.tier {
                    "degraded"
                } else {
                    "unchanged"
                }
            }
            None => "\u{2014}",
        }
    };

    // 1. Shadow evals — live during agent turns.
    for (path, eval) in &state.genome_shadow_evals {
        if is_gitignored(path) {
            continue;
        }
        let name = relative_file_path(path, &state.workspace_root);
        if !seen.insert(name.clone()) {
            continue;
        }
        file_rows.push(FileScoreRow {
            name,
            tier_ord: eval.tier as u8,
            tier: eval.tier.numeral().to_string(),
            quality: shadow_quality_with_reason(eval.quality, eval.tier, eval.consistency),
            consistency: cons_with_target(eval.tier, eval.consistency),
            delta: compute_delta(path, eval.tier).to_string(),
            is_shadow: true,
        });
    }

    // 2. Turn-modified files with genome reports (flattened across all agents).
    for paths in state.genome_turn_modified.values() {
        for path in paths {
            if is_gitignored(path) {
                continue;
            }
            let name = relative_file_path(path, &state.workspace_root);
            if !seen.insert(name.clone()) {
                continue;
            }
            if let Some(report) = state.genome_reports.get(path) {
                file_rows.push(FileScoreRow {
                    name,
                    tier_ord: report.tier as u8,
                    tier: report.tier.numeral().to_string(),
                    quality: quality_with_reason(report),
                    consistency: cons_with_target(report.tier, report.cross_encoder_consistency),
                    delta: compute_delta(path, report.tier).to_string(),
                    is_shadow: false,
                });
            }
        }
    }

    // 3. All persisted genome reports — these persist across turns.
    for (path, report) in &state.genome_reports {
        if is_gitignored(path) {
            continue;
        }
        let name = relative_file_path(path, &state.workspace_root);
        if !seen.insert(name.clone()) {
            continue;
        }
        file_rows.push(FileScoreRow {
            name,
            tier_ord: report.tier as u8,
            tier: report.tier.numeral().to_string(),
            quality: quality_with_reason(report),
            consistency: cons_with_target(report.tier, report.cross_encoder_consistency),
            delta: compute_delta(path, report.tier).to_string(),
            is_shadow: false,
        });
    }

    // Sort descending by tier (highest first), then by name for ties.
    file_rows.sort_by(|a, b| b.tier_ord.cmp(&a.tier_ord).then(a.name.cmp(&b.name)));

    if file_rows.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "No file changes detected",
            Style::default().fg(theme.border),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Files will appear here in real time as agents modify code.",
            dim_style,
        )));
        return lines;
    }

    // Section header.
    let file_count = file_rows.len();
    let header_text = format!("File Scores ({file_count} files) ");
    lines.push(Line::from(vec![
        Span::styled(header_text.clone(), label_style),
        Span::styled(
            "\u{2500}".repeat(width.saturating_sub(header_text.len())),
            dim_style,
        ),
    ]));

    // Fixed right-side columns — file name gets the remainder to show full paths.
    let tier_w = 4;
    let quality_w = 19; // "Failing (low c)" = 15, with padding
    let cons_w = 9; // "0.24/0.70"
    let delta_w = 9;
    let data_w = tier_w + quality_w + cons_w + delta_w + 4; // columns + gaps
    let name_w = width.saturating_sub(data_w).max(8);

    // Table header.
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {:<width$}", "FILE", width = name_w - 1),
            header_style,
        ),
        Span::styled(format!("{:>tier_w$} ", "TIER"), header_style),
        Span::styled(format!("{:<quality_w$}", "QUALITY"), header_style),
        Span::styled(format!("{:>cons_w$} ", "CONS/TARG"), header_style),
        Span::styled(format!("{:<delta_w$}", "DELTA"), header_style),
    ]));

    // Separator.
    lines.push(Line::from(Span::styled(
        "\u{2500}".repeat(width),
        dim_style,
    )));

    // Rows.
    for row in &file_rows {
        let delta_color = match row.delta.as_str() {
            "improved" => theme.success,
            "degraded" => theme.error,
            "unchanged" => theme.warning,
            _ => theme.border, // "—" (no baseline)
        };
        let quality_color = match row.quality.as_str() {
            "Exceptional" => theme.success,
            "Excellent" => theme.title_focused,
            "Standard" => theme.foreground,
            "Minimum" => theme.warning,
            _ => theme.error,
        };
        let live_marker = if row.is_shadow { "\u{25cf}" } else { " " };
        let display_name = truncate_str(&row.name, name_w.saturating_sub(1));

        lines.push(Line::from(vec![
            Span::styled(
                format!("{live_marker}{display_name:<width$}", width = name_w - 1),
                Style::default().fg(theme.foreground),
            ),
            Span::styled(
                format!("{:>tier_w$} ", row.tier),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<quality_w$}", row.quality),
                Style::default()
                    .fg(quality_color)
                    .add_modifier(if row.quality.contains('(') {
                        Modifier::DIM
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(
                format!("{:>cons_w$} ", row.consistency),
                Style::default().fg(theme.foreground),
            ),
            Span::styled(
                format!("{:<delta_w$}", row.delta),
                Style::default().fg(delta_color),
            ),
        ]));
    }

    lines
}

struct FileScoreRow {
    name: String,
    tier_ord: u8,
    tier: String,
    quality: String,
    consistency: String,
    delta: String,
    is_shadow: bool,
}

/// Format quality level with reason when it doesn't match what the tier suggests.
/// e.g. Tier IV + low consistency → "Failing (c)"
fn quality_with_reason(report: &GenomeReport) -> String {
    let level = report.quality_level();
    match report.quality_reason() {
        Some(reason) => format!("{level} ({reason})"),
        None => level.to_string(),
    }
}

/// Format consistency as "actual/target" so the operator sees the gap at a glance.
fn cons_with_target(tier: nit_core::GenomeTier, consistency: f32) -> String {
    let target = match tier {
        nit_core::GenomeTier::Replicator => 0.85,
        nit_core::GenomeTier::Methuselah => 0.70,
        nit_core::GenomeTier::Spaceship => 0.50,
        nit_core::GenomeTier::Oscillator => 0.25,
        nit_core::GenomeTier::StillLife => 0.25,
    };
    format!("{consistency:.2}/{target:.2}")
}

/// Same as quality_with_reason but for shadow evals (which only have tier + consistency).
fn shadow_quality_with_reason(
    quality: &str,
    tier: nit_core::GenomeTier,
    consistency: f32,
) -> String {
    // Check if tier is high enough but consistency drags it to Failing.
    let reason = match tier {
        nit_core::GenomeTier::Replicator if consistency < 0.85 => Some("low cons"),
        nit_core::GenomeTier::Methuselah if consistency < 0.70 => Some("low cons"),
        nit_core::GenomeTier::Spaceship if consistency < 0.50 => Some("low cons"),
        nit_core::GenomeTier::Oscillator if consistency < 0.25 => Some("low cons"),
        nit_core::GenomeTier::StillLife => Some("low tier"),
        _ => None,
    };
    match reason {
        Some(r) if quality == "Failing" || quality == "Minimum" => format!("{quality} ({r})"),
        _ => quality.to_string(),
    }
}

/// Get a display-friendly relative path from workspace root.
/// Tries multiple approaches to handle canonicalization mismatches on macOS.
fn relative_file_path(path: &std::path::Path, workspace: &std::path::Path) -> String {
    // 1. Direct strip_prefix.
    if let Ok(rel) = path.strip_prefix(workspace) {
        let s = rel.to_string_lossy();
        if !s.is_empty() {
            return s.to_string();
        }
    }
    // 2. Canonicalize both to resolve symlinks (e.g., /var vs /private/var on macOS).
    if let (Ok(canon_path), Ok(canon_ws)) = (path.canonicalize(), workspace.canonicalize()) {
        if let Ok(rel) = canon_path.strip_prefix(&canon_ws) {
            let s = rel.to_string_lossy();
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    // 3. String-based stripping as last structured attempt.
    let path_s = path.to_string_lossy();
    let ws_s = workspace.to_string_lossy();
    if let Some(rest) = path_s.strip_prefix(ws_s.as_ref()) {
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        if !rest.is_empty() {
            return rest.to_string();
        }
    }
    // 4. Full path.
    path_s.to_string()
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else if max > 2 {
        format!("{}\u{2026}", &s[..max - 1])
    } else {
        s[..max].to_string()
    }
}
