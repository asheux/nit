//! Gate Monitor pane — dual-mode widget that shows either the code-genome
//! structural-quality dashboard (editor apps) or a per-file score table
//! (FILESCORES sub-view), plus a compact status view in Games mode.

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
use crate::widgets::text_utils::truncate_with_ellipsis;
use crate::workspace_scan::{WorkspaceScanItemState, WorkspaceScanRuntime};

// Title layout: " CODE STRUCTURAL QUALITY [NxN] " ++ " STATS " ++ " FILESCORES " ++ " LIVE " ++ "  EVALUATE GENOME  "
const BTN_STATS_LABEL: &str = " STATS ";
const BTN_FILESCORES_LABEL: &str = " FILESCORES ";
const BTN_LIVE_LABEL: &str = " LIVE ";
// EVAL button uses an explicit verb-noun label so the operator knows the
// click triggers a full-project genome evaluation (not a tab switch).
// Counts ("stale/total") trail the verb so the operator can see at a
// glance how much work the click would queue without doing it. The bare
// labels are the fallback when no dry walk has run yet.
const BTN_EVAL_LABEL_IDLE: &str = " ▶ EVALUATE GENOME ";
const BTN_EVAL_LABEL_CLEAN: &str = " ✓ ALL EVALUATED ";

const STATS_LABEL_WIDTH: usize = 12;
const STATS_GAUGE_LABEL_WIDTH: usize = 14;
// Above this generation count the encoder bar reads as "fully healthy" (Tier V endurance).
const ENCODER_GEN_SATURATION: f32 = 3000.0;
const DENSITY_WARN: f32 = 0.35;
const DENSITY_OVER: f32 = 0.45;
const GAUGE_ACCENT_THRESHOLD: f32 = 0.6;
const GAUGE_MID_THRESHOLD: f32 = 0.3;
const TIER_LEVELS: usize = 5;
const LOADING_PERIOD_MS: f64 = 1600.0;

// `title_prefix_len` is the display width of the dynamic prefix (varies
// with grid size). `eval_label_width` is the display width of the EVAL
// button's rendered label, which varies because it flips to
// "EVALUATING N/M" while a scan is in flight and includes multibyte
// glyphs (▶ / ⟳). Each button returns a direct-set action — returning a
// single "toggle" action for the three sub-view tabs would collapse to a
// cycle and take the user to the wrong tab when they click a non-adjacent
// one.
pub fn title_button_hit(
    col_in_rect: u16,
    title_prefix_len: u16,
    eval_label_width: u16,
) -> Option<Action> {
    let col = col_in_rect.saturating_sub(1); // border offset
    let stats_start = title_prefix_len + 1; // space separator
    let stats_end = stats_start + BTN_STATS_LABEL.len() as u16;
    let fs_start = stats_end + 1;
    let fs_end = fs_start + BTN_FILESCORES_LABEL.len() as u16;
    let live_start = fs_end + 1;
    let live_end = live_start + BTN_LIVE_LABEL.len() as u16;
    // Double-space gap before EVAL to visually separate the action button
    // from the sub-view tabs.
    let eval_start = live_end + 2;
    let eval_end = eval_start + eval_label_width;
    if (stats_start..stats_end).contains(&col) {
        Some(Action::GateMonitorSetSubView(GateMonitorSubView::Stats))
    } else if (fs_start..fs_end).contains(&col) {
        Some(Action::GateMonitorSetSubView(
            GateMonitorSubView::FileScores,
        ))
    } else if (live_start..live_end).contains(&col) {
        Some(Action::GateMonitorSetSubView(GateMonitorSubView::Live))
    } else if (eval_start..eval_end).contains(&col) {
        Some(Action::WorkspaceScanStart)
    } else {
        None
    }
}

/// Renders the EVAL button label. Three states:
/// - scanning: "⟳ EVALUATING done/queued" so the operator gets live
///   feedback inside the button itself.
/// - clean (dry walk found nothing stale, or scan just drained):
///   "✓ ALL EVALUATED total" — visually muted, still clickable for
///   re-verification.
/// - idle: "▶ EVALUATE GENOME stale/total" — green; the trailing
///   "stale/total" tells the operator how many files the click would
///   queue before they commit.
///
/// Returns an owned String so the caller can use the character width for
/// hit-testing without re-computing.
pub fn eval_button_label(state: &AppState) -> String {
    if let Some((done, queued)) = state.agents.workspace_scan_progress {
        format!(" ⟳ EVALUATING {done}/{queued} ")
    } else if state.agents.workspace_scan_clean {
        let total = state.agents.workspace_scan_total_files;
        if total > 0 {
            format!(" ✓ ALL EVALUATED {total} ")
        } else {
            BTN_EVAL_LABEL_CLEAN.to_string()
        }
    } else {
        let stale = state.agents.workspace_scan_stale_files;
        let total = state.agents.workspace_scan_total_files;
        if total > 0 {
            format!(" ▶ EVALUATE GENOME {stale}/{total} ")
        } else {
            // No dry walk has run yet (or the workspace has no code files).
            // Fall back to the bare verb so we don't render "0/0".
            BTN_EVAL_LABEL_IDLE.to_string()
        }
    }
}

/// Display width (in terminal cells) of a string. Use this for hit-test
/// math instead of `len()` — `len()` counts bytes, which over-counts
/// multibyte glyphs like ▶ / ⟳ and would shift the EVAL button's hit
/// region right by a few cells, breaking clicks on the trailing edge.
pub fn display_width(s: &str) -> u16 {
    use unicode_width::UnicodeWidthStr;
    s.width() as u16
}

pub fn render(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &mut AppState,
    workspace_scan: &WorkspaceScanRuntime,
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

    let sub_view = state.gate_monitor_sub_view;
    let stats_style = if sub_view == GateMonitorSubView::Stats {
        btn_active
    } else {
        btn_inactive
    };
    let fs_style = if sub_view == GateMonitorSubView::FileScores {
        btn_active
    } else {
        btn_inactive
    };
    let live_style = if sub_view == GateMonitorSubView::Live {
        btn_active
    } else {
        btn_inactive
    };
    let eval_label = eval_button_label(state);
    let scanning = state.agents.workspace_scan_progress.is_some();
    let clean = !scanning && state.agents.workspace_scan_clean;
    // EVAL button is intentionally distinct from the STATS / FILESCORES /
    // LIVE sub-view tabs because it does something — kicks off a CPU-heavy
    // genome scan of every file in the workspace — rather than just
    // switching sub-views.
    //   - amber (warning): scan in flight; "working, leave me alone"
    //   - muted border-on-bg + DIM: cache is clean; clicking re-verifies
    //     but probably finds nothing
    //   - green (success): ready to evaluate; "click me to scan"
    let eval_style = if scanning {
        Style::default()
            .fg(theme.background)
            .bg(theme.warning)
            .add_modifier(Modifier::BOLD)
    } else if clean {
        Style::default()
            .fg(theme.border)
            .bg(theme.background)
            .add_modifier(Modifier::DIM)
    } else {
        Style::default()
            .fg(theme.background)
            .bg(theme.success)
            .add_modifier(Modifier::BOLD)
    };

    let title = Line::from(vec![
        Span::styled(title_prefix, title_style),
        Span::styled(" ", sep_style),
        Span::styled(BTN_STATS_LABEL, stats_style),
        Span::styled(" ", sep_style),
        Span::styled(BTN_FILESCORES_LABEL, fs_style),
        Span::styled(" ", sep_style),
        Span::styled(BTN_LIVE_LABEL, live_style),
        Span::styled("  ", sep_style),
        Span::styled(eval_label, eval_style),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .border_type(border_type)
        .style(Style::default().bg(theme.background))
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if genome_report.is_none() && state.genome_computing {
        draw_loading_bar(frame, inner, theme);
        return;
    }

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
        GateMonitorSubView::Live => {
            build_lines_live(state, workspace_scan, theme, inner.width as usize)
        }
    };
    let lines = apply_ui_selection(
        lines,
        state.ui_selection.as_ref(),
        UiSelectionPane::GateMonitor,
        theme.selection_bg,
        0,
    );
    // Cache max_scroll + clamp stored offset so scroll handlers can skip
    // rebuilding the genome report on every wheel tick.
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    let scroll = state.gate_monitor_scroll.min(max_scroll);
    state.gate_monitor_last_max_scroll = max_scroll;
    state.gate_monitor_scroll = scroll;
    let lines: Vec<Line<'static>> = lines
        .into_iter()
        .map(crate::widgets::tab_expand::expand_tabs_in_line)
        .collect();
    let para = Paragraph::new(lines)
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .scroll((scroll as u16, 0));
    frame.render_widget(para, inner);
}

// Used by tests and scroll-clamp callers that need the line count without a
// frame. `workspace_scan` is optional because some callers (mouse handlers
// doing pre-render scroll clamp math) don't thread it — the Live sub-view
// falls back to an empty result in that case and the cached max_scroll
// catches up on the next render.
pub fn build_lines(
    state: &AppState,
    workspace_scan: Option<&WorkspaceScanRuntime>,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
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
            GateMonitorSubView::Live => match workspace_scan {
                Some(ws) => build_lines_live(state, ws, theme, width),
                None => Vec::new(),
            },
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
    let display_delta = display_quality_delta(state, report);
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

// Compares against the session baseline for this file; falls back to the
// state-level delta when no baseline exists (first-ever evaluation).
// Derived from current report data so async eval timing can't desync it.
fn display_quality_delta(state: &AppState, report: &GenomeReport) -> i32 {
    let Some(file_path) = state.editor_buffer().path() else {
        return state.genome_quality_delta;
    };
    let Some(base) = state.genome_baselines.get(file_path) else {
        return state.genome_quality_delta;
    };
    if report.tier > base.tier {
        return 1;
    }
    if report.tier < base.tier {
        return -1;
    }
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

fn build_tier_bar(tier: nit_core::GenomeTier, width: usize, theme: &Theme) -> Line<'static> {
    if width < TIER_LEVELS {
        return Line::from("");
    }
    let tier_val = tier as u32; // 0..4
    let filled = ((tier_val + 1) as usize * width) / TIER_LEVELS;
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

// Renders: "Label  ▓▓▓▓░░░░░░░░  0.23"
fn build_gauge_line(label: &str, value: f32, width: usize, theme: &Theme) -> Line<'static> {
    let num_str = format!("{value:.2}");
    let num_w = num_str.len() + 1;
    let bar_w = width.saturating_sub(STATS_GAUGE_LABEL_WIDTH + num_w + 1);

    let clamped = value.clamp(0.0, 1.0);
    let filled = (clamped * bar_w as f32).round() as usize;
    let empty = bar_w.saturating_sub(filled);

    let bar_color = if clamped >= GAUGE_ACCENT_THRESHOLD {
        theme.accent
    } else if clamped >= GAUGE_MID_THRESHOLD {
        theme.title_focused
    } else {
        theme.title
    };

    Line::from(vec![
        Span::styled(
            pad_to_width(label, STATS_GAUGE_LABEL_WIDTH),
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

fn build_encoder_line(
    score: &nit_core::EncoderScore,
    width: usize,
    theme: &Theme,
    is_ast: bool,
) -> Line<'static> {
    let name = encoder_short_name(score.encoder);
    let gen = score.generations_survived;
    let gen_str = gen.to_string();
    let stats_str = format!(
        " density={:.2} components={}",
        score.density, score.components
    );
    let fixed_w = STATS_LABEL_WIDTH + gen_str.len() + stats_str.len() + 2;
    let bar_w = width.saturating_sub(fixed_w);

    let ratio = (gen as f32 / ENCODER_GEN_SATURATION).clamp(0.0, 1.0);
    let filled = (ratio * bar_w as f32).round() as usize;
    let empty = bar_w.saturating_sub(filled);
    let bar_color = gen_color(gen, theme);

    let name_style = if is_ast {
        Style::default().fg(theme.title_focused)
    } else {
        Style::default().fg(theme.title).add_modifier(Modifier::DIM)
    };

    Line::from(vec![
        Span::styled(pad_to_width(name, STATS_LABEL_WIDTH), name_style),
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

fn build_density_line(
    score: &nit_core::EncoderScore,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let name = encoder_short_name(score.encoder);
    let val_str = format!("{:.2}", score.density);
    let fixed_w = STATS_LABEL_WIDTH + val_str.len() + 2;
    let bar_w = width.saturating_sub(fixed_w);

    let clamped = score.density.clamp(0.0, 1.0);
    let filled = (clamped * bar_w as f32).round() as usize;
    let empty = bar_w.saturating_sub(filled);

    let over_threshold = score.density > DENSITY_OVER;
    let bar_color = if over_threshold {
        theme.foreground
    } else if score.density > DENSITY_WARN {
        theme.title
    } else {
        theme.title_focused
    };

    let mut spans = vec![
        Span::styled(
            pad_to_width(name, STATS_LABEL_WIDTH),
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

fn build_sim_detail_line(
    score: &nit_core::EncoderScore,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let name = encoder_short_name(score.encoder);
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
    let used = STATS_LABEL_WIDTH + detail.len();
    let pad = width.saturating_sub(used);

    Line::from(vec![
        Span::styled(
            pad_to_width(name, STATS_LABEL_WIDTH),
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

fn draw_no_file_placeholder(frame: &mut Frame, area: ratatui::layout::Rect, theme: &Theme) {
    if area.width < 4 || area.height < 1 {
        return;
    }
    let msg = "Open a file in the editor to view its code genome";
    let msg_len = msg.chars().count() as u16;
    if msg_len > area.width.saturating_sub(2) {
        return;
    }
    let text_x = area.x + area.width.saturating_sub(msg_len) / 2;
    let text_y = area.y + area.height / 2;
    let style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);

    let buf = frame.buffer_mut();
    for (i, ch) in msg.chars().enumerate() {
        let cx = text_x + i as u16;
        if cx < area.x + area.width {
            let cell = buf.get_mut(cx, text_y);
            cell.set_char(ch);
            cell.set_style(style);
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

// Triangular wave 0→1→0 over LOADING_PERIOD_MS, driven by wall clock.
fn loading_ratio() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let millis = now.as_millis() as f64;
    let phase = (millis % LOADING_PERIOD_MS) / LOADING_PERIOD_MS;
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
    for (file_path, cached_report) in &state.genome_reports {
        if is_gitignored(file_path) {
            continue;
        }
        let name = relative_file_path(file_path, &state.workspace_root);
        if !seen.insert(name.clone()) {
            continue;
        }
        file_rows.push(FileScoreRow {
            name,
            tier_ord: cached_report.tier as u8,
            tier: cached_report.tier.numeral().to_string(),
            quality: quality_with_reason(cached_report),
            consistency: cons_with_target(
                cached_report.tier,
                cached_report.cross_encoder_consistency,
            ),
            delta: compute_delta(file_path, cached_report.tier).to_string(),
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
        let display_name = truncate_with_ellipsis(&row.name, name_w.saturating_sub(1));

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

// Surface the *why* when quality trails the tier (e.g. Tier IV + low consistency → "Failing (c)").
fn quality_with_reason(report: &GenomeReport) -> String {
    let level = report.quality_level();
    match report.quality_reason() {
        Some(reason) => format!("{level} ({reason})"),
        None => level.to_string(),
    }
}

// "actual/target" form surfaces the tier gap at a glance.
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

// Shadow evals only expose tier+consistency, so derive the reason locally.
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

// macOS canonicalizes /var → /private/var, so strip_prefix alone isn't enough.
fn relative_file_path(path: &std::path::Path, workspace: &std::path::Path) -> String {
    if let Ok(rel) = path.strip_prefix(workspace) {
        let s = rel.to_string_lossy();
        if !s.is_empty() {
            return s.to_string();
        }
    }
    if let (Ok(canon_path), Ok(canon_ws)) = (path.canonicalize(), workspace.canonicalize()) {
        if let Ok(rel) = canon_path.strip_prefix(&canon_ws) {
            let s = rel.to_string_lossy();
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    let path_s = path.to_string_lossy();
    let ws_s = workspace.to_string_lossy();
    if let Some(rest) = path_s.strip_prefix(ws_s.as_ref()) {
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        if !rest.is_empty() {
            return rest.to_string();
        }
    }
    path_s.to_string()
}

// ---------------------------------------------------------------------------
// LIVE sub-view: workspace-scan queue (evaluating + queued, session-local)
// ---------------------------------------------------------------------------

/// Rendering-local state tag: unifies workspace-scan queue items with
/// in-flight agent-turn evaluations, current-turn file edits, and
/// already-completed entries from this session. The enum doesn't leak
/// into `WorkspaceScanRuntime` because the runtime itself doesn't know
/// about agent turns — the unification happens at display time so the
/// LIVE view shows "what nit is doing right now" PLUS "what nit has
/// evaluated this session".
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum LiveItemKind {
    /// In workspace_scan.dispatched — background eval thread running.
    ScanEvaluating,
    /// In workspace_scan.pending — waiting for an in-flight slot.
    ScanQueued,
    /// Agent turn's authoritative eval batch is still draining for this file.
    AgentEvaluating,
    /// Agent is currently editing this file — the turn is active and wrote
    /// to it, but the post-turn eval hasn't dispatched yet.
    AgentEditing,
    /// Session-touched but no longer in any active pipeline — the eval has
    /// landed and the file sits in `state.genome_reports`. Kept visible so
    /// the operator can see a running log of what was evaluated.
    Done,
}

impl LiveItemKind {
    fn label(self) -> &'static str {
        match self {
            LiveItemKind::ScanEvaluating => "evaluating",
            LiveItemKind::ScanQueued => "queued",
            LiveItemKind::AgentEvaluating => "agent-eval",
            LiveItemKind::AgentEditing => "editing",
            LiveItemKind::Done => "done",
        }
    }

    fn marker(self) -> &'static str {
        match self {
            // ●: actively running work
            LiveItemKind::ScanEvaluating | LiveItemKind::AgentEvaluating => "\u{25cf}",
            // ◐: agent-modified but not yet evaluated
            LiveItemKind::AgentEditing => "\u{25d0}",
            // ○: queued
            LiveItemKind::ScanQueued => "\u{25cb}",
            // ✓: done
            LiveItemKind::Done => "\u{2713}",
        }
    }
}

/// Precedence when the same path appears in multiple sources. Higher number
/// wins — show the most-active state so the operator sees what's happening
/// now, not a stale lower-priority tag.
fn live_item_priority(kind: LiveItemKind) -> u8 {
    match kind {
        LiveItemKind::ScanEvaluating => 5,
        LiveItemKind::AgentEvaluating => 4,
        LiveItemKind::AgentEditing => 3,
        LiveItemKind::ScanQueued => 2,
        LiveItemKind::Done => 1,
    }
}

fn build_lines_live(
    state: &AppState,
    workspace_scan: &WorkspaceScanRuntime,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let header_style = Style::default()
        .fg(theme.title)
        .add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Unify all in-flight sources into a single deduped map keyed by path.
    // Higher-priority state wins when the same file appears in multiple
    // sources (e.g., an agent-eval batch is running on a path the background
    // scan also queued — show "agent-eval", not "queued").
    let mut combined: std::collections::HashMap<std::path::PathBuf, LiveItemKind> =
        std::collections::HashMap::new();
    let upsert = |combined: &mut std::collections::HashMap<_, LiveItemKind>,
                  path: std::path::PathBuf,
                  kind: LiveItemKind| {
        let entry = combined.entry(path).or_insert(kind);
        if live_item_priority(kind) > live_item_priority(*entry) {
            *entry = kind;
        }
    };

    // 1. Workspace-scan pending + dispatched.
    for (path, scan_state) in workspace_scan.in_flight_snapshot() {
        let kind = match scan_state {
            WorkspaceScanItemState::Evaluating => LiveItemKind::ScanEvaluating,
            WorkspaceScanItemState::Queued => LiveItemKind::ScanQueued,
        };
        upsert(&mut combined, path, kind);
    }

    // 2. Agent-turn authoritative eval batches that are still draining.
    for (agent_id, batch) in &state.genome_eval_batches {
        if batch.pending == 0 {
            continue;
        }
        if let Some(paths) = state.genome_turn_modified.get(agent_id) {
            for path in paths {
                upsert(&mut combined, path.clone(), LiveItemKind::AgentEvaluating);
            }
        }
    }

    // 3. Files an agent is currently editing mid-turn (no batch yet because
    //    the turn hasn't completed). These give the operator immediate
    //    feedback the moment a FileWrite event fires, without waiting for
    //    the 200 ms file-watcher poll or the post-turn dispatch.
    for agent_id in &state.genome_turn_active {
        let batch_empty = state
            .genome_eval_batches
            .get(agent_id)
            .map(|b| b.pending == 0)
            .unwrap_or(true);
        if !batch_empty {
            // Already covered by AgentEvaluating above — skip to avoid
            // downgrading the state tag.
            continue;
        }
        if let Some(paths) = state.genome_turn_modified.get(agent_id) {
            for path in paths {
                upsert(&mut combined, path.clone(), LiveItemKind::AgentEditing);
            }
        }
    }

    // 4. Session log: every path agents have touched this session. These
    //    show up as "done" once their eval batch drains — the entry stays
    //    visible for the rest of the session so the operator gets a
    //    running log of what was actually modified. `genome_turn_modified`
    //    is runtime-only (#[serde(skip)]) so it resets on every nit
    //    launch: LIVE is truly session-scoped, not carried across runs.
    for paths in state.genome_turn_modified.values() {
        for path in paths {
            combined.entry(path.clone()).or_insert(LiveItemKind::Done);
        }
    }

    if combined.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "No files are being evaluated",
            Style::default().fg(theme.border),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Files appear here in real time as agents edit code or the",
            dim_style,
        )));
        lines.push(Line::from(Span::styled(
            "workspace scan picks up file-watcher events.",
            dim_style,
        )));
        return lines;
    }

    // Sort: highest priority first, then alphabetically so rows stay stable
    // across ticks.
    let mut items: Vec<(std::path::PathBuf, LiveItemKind)> = combined.into_iter().collect();
    items.sort_by(|a, b| {
        live_item_priority(b.1)
            .cmp(&live_item_priority(a.1))
            .then_with(|| a.0.cmp(&b.0))
    });

    let (_scan_done, scan_total) = workspace_scan.progress();
    let scan_eval = items
        .iter()
        .filter(|(_, k)| matches!(k, LiveItemKind::ScanEvaluating))
        .count();
    let agent_eval = items
        .iter()
        .filter(|(_, k)| matches!(k, LiveItemKind::AgentEvaluating))
        .count();
    let editing = items
        .iter()
        .filter(|(_, k)| matches!(k, LiveItemKind::AgentEditing))
        .count();
    let queued = items
        .iter()
        .filter(|(_, k)| matches!(k, LiveItemKind::ScanQueued))
        .count();
    let done_count = items
        .iter()
        .filter(|(_, k)| matches!(k, LiveItemKind::Done))
        .count();
    let active = scan_eval + agent_eval + editing + queued;

    let header_text =
        format!("Live Session ({active} active, {done_count} done / {scan_total} scanned) ");
    lines.push(Line::from(vec![
        Span::styled(header_text.clone(), label_style),
        Span::styled(
            "\u{2500}".repeat(width.saturating_sub(header_text.len())),
            dim_style,
        ),
    ]));

    // Columns: FILE | STATUS | TIER. For active entries the tier shows
    // whatever's left of the previous run's cache (before invalidation);
    // for Done entries it's the freshly-computed tier from this session.
    let status_w = 12;
    let prev_w = 12;
    let data_w = status_w + prev_w + 2;
    let name_w = width.saturating_sub(data_w).max(8);

    lines.push(Line::from(vec![
        Span::styled(
            format!(" {:<width$}", "FILE", width = name_w - 1),
            header_style,
        ),
        Span::styled(format!("{:<status_w$}", "STATUS"), header_style),
        Span::styled(format!("{:<prev_w$}", "TIER"), header_style),
    ]));
    lines.push(Line::from(Span::styled(
        "\u{2500}".repeat(width),
        dim_style,
    )));

    for (path, kind) in &items {
        let rel = relative_file_path(path, &state.workspace_root);
        let display_name = truncate_with_ellipsis(&rel, name_w.saturating_sub(2));
        let marker = kind.marker();
        let status_color = match kind {
            LiveItemKind::ScanEvaluating | LiveItemKind::AgentEvaluating => theme.accent,
            LiveItemKind::AgentEditing => theme.title_focused,
            LiveItemKind::ScanQueued => theme.title,
            LiveItemKind::Done => theme.success,
        };
        // Done entries show the current tier (evaluated this session);
        // in-flight entries fall back to the session baseline since their
        // cache was invalidated when they entered the queue.
        let tier_display = state
            .genome_reports
            .get(path)
            .or_else(|| state.genome_baselines.get(path))
            .map(|r| format!("tier {}", r.tier.numeral()))
            .unwrap_or_else(|| "\u{2014}".to_string()); // —

        // Dim the whole row for completed entries so the eye finds active
        // work first.
        let name_style = if matches!(kind, LiveItemKind::Done) {
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(theme.foreground)
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("{marker} {display_name:<width$}", width = name_w - 2),
                name_style,
            ),
            Span::styled(
                format!("{:<status_w$}", kind.label()),
                Style::default().fg(status_color),
            ),
            Span::styled(
                format!("{tier_display:<prev_w$}"),
                Style::default().fg(theme.border),
            ),
        ]));
    }

    lines
}
