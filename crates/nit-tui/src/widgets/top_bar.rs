use std::time::Duration;

use nit_core::AppState;
use ratatui::{
    layout::Alignment,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::Theme;
use crate::vitals::{LabCriticality, LabVitalsSnapshot};

pub fn render(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    vitals: &LabVitalsSnapshot,
) {
    let mode = format!("{:?}", state.mode).to_uppercase();
    let app_label = state.app_kind.label();
    let file = state
        .editor_buffer()
        .path()
        .map(|p| p.display().to_string());
    let dirty = if state.editor_buffer().is_dirty() {
        "*"
    } else {
        ""
    };
    let buffer_name = state.editor_buffer().name().to_string();
    let file_text = match file {
        Some(path) => path,
        None if buffer_name == "untitled" => state.file_tree.root.display().to_string(),
        None => buffer_name,
    };
    let status_text_raw = state.status.as_deref().unwrap_or_default();
    let status_text = if state.debug {
        status_text_raw.to_string()
    } else {
        minify_status_text(status_text_raw)
    };
    let compact_status_mode =
        matches!(state.app_kind, nit_core::AppKind::Games) && state.games.running;
    let status_label = build_status_label(&status_text, compact_status_mode);

    // Mood badge — living-system global modulator. Placed at top-left so it
    // is always visible; color-coded per mood state.
    let mood_label: &'static str = match state.substrate.mood {
        nit_core::mood::Mood::Exploration => "EXPLORATION",
        nit_core::mood::Mood::Consolidation => "CONSOLIDATION",
        nit_core::mood::Mood::Defensive => "DEFENSIVE",
    };
    let mood_color = match state.substrate.mood {
        nit_core::mood::Mood::Exploration => theme.success,
        nit_core::mood::Mood::Consolidation => theme.accent,
        nit_core::mood::Mood::Defensive => theme.warning,
    };
    let mood_segment = format!(" | MOOD: {mood_label} ");

    let inner_width = area.width.saturating_sub(2) as usize;
    let fixed_width = [
        " nit ",
        " | ",
        " | ",
        &mode,
        " | ",
        app_label,
        " | UTF-8 ",
        &mood_segment,
    ]
    .iter()
    .map(|s| s.width())
    .sum::<usize>();
    let min_vitals_width = 26usize;
    let left_budget = inner_width.saturating_sub(min_vitals_width + 1);
    let file_max = left_budget.saturating_sub(fixed_width);

    let file_display = if dirty.is_empty() {
        truncate_start(&file_text, file_max)
    } else {
        let star_width = "*".width();
        let name_max = file_max.saturating_sub(star_width);
        truncate_start(&file_text, name_max)
    };

    let mut spans = vec![
        Span::styled(
            " nit ",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | ", Style::default().fg(theme.border)),
        Span::styled(
            file_display,
            Style::default()
                .fg(if dirty.is_empty() {
                    theme.foreground
                } else {
                    theme.title_focused
                })
                .add_modifier(if dirty.is_empty() {
                    Modifier::empty()
                } else {
                    Modifier::BOLD
                }),
        ),
    ];
    if !dirty.is_empty() {
        spans.push(Span::styled(
            "*",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.extend([
        Span::styled(" | ", Style::default().fg(theme.border)),
        Span::styled(mode, Style::default().fg(theme.title)),
        Span::styled(" | ", Style::default().fg(theme.border)),
        Span::styled(app_label, Style::default().fg(theme.title)),
        Span::styled(" | UTF-8", Style::default().fg(theme.border)),
        Span::styled(" | ", Style::default().fg(theme.border)),
        Span::styled("MOOD: ", Style::default().fg(theme.border)),
        Span::styled(
            mood_label,
            Style::default().fg(mood_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::default().fg(theme.border)),
    ]);

    let left_width: usize = spans.iter().map(|s| s.content.as_ref().width()).sum();
    let max_right = inner_width.saturating_sub(left_width).saturating_sub(1);
    let (mut vitals_spans, mut right_width) = build_vitals_spans(vitals, theme, max_right);
    if !status_label.is_empty() && max_right > right_width + 4 {
        let status_max = max_right.saturating_sub(right_width + 2);
        let trimmed = truncate_start(&status_label, status_max);
        if !trimmed.is_empty() {
            right_width += 2 + trimmed.width();
            vitals_spans.push(Span::raw("  "));
            vitals_spans.push(Span::styled(trimmed, status_style(&status_text, theme)));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " NEURAL INTERFACE TERMINAL ",
            Style::default()
                .fg(theme.title)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background));

    if right_width > 0 && left_width < inner_width {
        let pad = inner_width
            .saturating_sub(left_width)
            .saturating_sub(right_width);
        let gap = if pad == 0 { 1 } else { pad };
        spans.push(Span::raw(" ".repeat(gap)));
        spans.extend(vitals_spans);
    }

    let line = Line::from(spans);
    let para = Paragraph::new(line)
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .alignment(Alignment::Left)
        .block(block);

    frame.render_widget(para, area);
}

fn build_vitals_spans(
    vitals: &LabVitalsSnapshot,
    theme: &Theme,
    max_width: usize,
) -> (Vec<Span<'static>>, usize) {
    if max_width == 0 {
        return (Vec::new(), 0);
    }

    let hb_text = if vitals.job_running {
        format_duration(vitals.hb_age)
    } else {
        "idle".to_string()
    };
    let ag_text = if vitals.agent_enabled && vitals.agent_connected {
        format_duration(vitals.ag_age)
    } else {
        "--".to_string()
    };
    let level = vitals.criticality.label().to_string();
    let level_style = criticality_style(vitals.criticality, theme);
    let ecg_style = match vitals.criticality {
        LabCriticality::Crit => Style::default()
            .fg(theme.error)
            .add_modifier(Modifier::BOLD),
        _ => level_style,
    };
    let label_style = theme.status_idle_style();
    let number_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let hb_style = if vitals.job_running {
        number_style
    } else {
        theme.status_idle_style()
    };
    let ag_style = if vitals.agent_enabled && vitals.agent_connected {
        number_style
    } else {
        theme.status_idle_style()
    };

    let prefix_width = "LAB ".width() + level.width() + "  ECG ".width();
    let max_ecg_width = 28usize;
    let details = [
        VitalsDetails::Full,
        VitalsDetails::HeartbeatOnly,
        VitalsDetails::Compact,
    ];
    for details in details {
        let suffix_width = match details {
            VitalsDetails::Full => {
                "  HB ".width() + hb_text.width() + "  AG ".width() + ag_text.width()
            }
            VitalsDetails::HeartbeatOnly => "  HB ".width() + hb_text.width(),
            VitalsDetails::Compact => 0,
        };
        let min_ecg = if matches!(details, VitalsDetails::Compact) {
            6
        } else {
            10
        };
        if max_width < prefix_width + suffix_width + min_ecg {
            continue;
        }
        let ecg_width = max_width
            .saturating_sub(prefix_width + suffix_width)
            .min(max_ecg_width)
            .max(min_ecg);
        let ecg = vitals.severity_scaled_waveform(ecg_width);
        let mut spans = vec![
            Span::styled("LAB ", label_style),
            Span::styled(level.clone(), level_style),
            Span::styled("  ECG ", label_style),
            Span::styled(ecg, ecg_style),
        ];
        match details {
            VitalsDetails::Full => {
                spans.push(Span::styled("  HB ", label_style));
                spans.push(Span::styled(hb_text.clone(), hb_style));
                spans.push(Span::styled("  AG ", label_style));
                spans.push(Span::styled(ag_text.clone(), ag_style));
            }
            VitalsDetails::HeartbeatOnly => {
                spans.push(Span::styled("  HB ", label_style));
                spans.push(Span::styled(hb_text.clone(), hb_style));
            }
            VitalsDetails::Compact => {}
        }
        let width = spans.iter().map(|s| s.content.as_ref().width()).sum();
        return (spans, width);
    }

    let fallback = truncate_start(&format!("LAB {level}"), max_width);
    let width = fallback.width();
    (vec![Span::styled(fallback, level_style)], width)
}

fn truncate_start(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let mut width = 0;
    let mut idx = text.len();
    for (i, ch) in text.char_indices().rev() {
        width += UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        if width >= max_width.saturating_sub(1) {
            idx = i;
            break;
        }
    }
    format!("…{}", &text[idx..])
}

fn format_duration(age: Option<Duration>) -> String {
    let Some(age) = age else {
        return "--".to_string();
    };
    let secs = age.as_secs_f64();
    if secs < 9.95 {
        if secs > 0.0 && secs < 0.1 {
            "0.1s".to_string()
        } else {
            format!("{secs:.1}s")
        }
    } else if secs < 60.0 {
        format!("{secs:.0}s")
    } else if secs < 3600.0 {
        format!("{:.0}m", secs / 60.0)
    } else {
        format!("{:.0}h", secs / 3600.0)
    }
}

fn criticality_style(level: LabCriticality, theme: &Theme) -> Style {
    match level {
        LabCriticality::Idle => theme.status_idle_style(),
        LabCriticality::Ok => theme.status_ok_style(),
        LabCriticality::Warn => theme.status_warn_style(),
        LabCriticality::Hot => theme.status_hot_style(),
        LabCriticality::Crit => theme.status_crit_style(),
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum VitalsDetails {
    Full,
    HeartbeatOnly,
    Compact,
}

fn status_style(status: &str, theme: &Theme) -> Style {
    let lower = status.to_ascii_lowercase();
    let color = if lower.contains("error")
        || lower.contains("failed")
        || lower.contains("invalid")
        || lower.contains("unknown")
    {
        theme.error
    } else {
        theme.foreground
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn minify_status_text(status_text: &str) -> String {
    let text = status_text.trim();
    if text.is_empty() {
        return String::new();
    }

    // A file open already updates the file label elsewhere (top-left), so avoid redundant paths.
    if text.starts_with("Opened ") {
        return "Opened".into();
    }

    // Keep the first clause for multi-part guidance strings.
    if let Some((left, _)) = text.split_once(" - ") {
        return left.trim().to_string();
    }

    // Prefer terse status verbs over repeating the chosen option/value.
    if text.starts_with("Protocol set to ") {
        return "Protocol set".into();
    }
    if text.starts_with("GoL rule set to ") {
        return "GoL rule set".into();
    }

    // If there's a long detail payload after a colon, keep just the label.
    if let Some((left, right)) = text.split_once(':') {
        let right = right.trim();
        if right.is_empty() {
            return left.trim().to_string();
        }
        let too_long = right.len() > 16;
        let has_pathish = right.contains('/') || right.contains('\\');
        let word_count = right.split_whitespace().count();
        if too_long || has_pathish || word_count > 3 {
            return left.trim().to_string();
        }
    }

    // Drop trailing parenthetical detail (usually debug-ish).
    if let Some((left, _)) = text.split_once(" (") {
        return left.trim().to_string();
    }

    text.to_string()
}

fn build_status_label(status_text: &str, compact_mode: bool) -> String {
    let text = status_text.trim();
    if text.is_empty() {
        return String::new();
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains("error")
        || lower.contains("failed")
        || lower.contains("invalid")
        || lower.contains("unknown")
        || lower.contains("panic")
        || lower.contains("fatal")
        || lower.contains("timeout")
        || lower.contains("crash")
    {
        return "STATUS: ERROR".into();
    }
    if compact_mode {
        let compact = if lower.contains("queued") {
            Some("QUEUED")
        } else if lower.contains("running")
            || lower.contains("loading")
            || lower.contains("pending")
            || lower.contains("preparing")
            || lower.contains("started")
        {
            Some("BUSY")
        } else if lower.contains("completed")
            || lower.contains("complete")
            || lower.contains("done")
        {
            Some("DONE")
        } else if lower.contains("cancelled") || lower.contains("canceled") {
            Some("CANCELED")
        } else if lower.contains("stopping")
            || lower.contains("stopped")
            || lower.contains("closing")
        {
            Some("STOPPING")
        } else {
            Some("ACTIVE")
        };
        if let Some(label) = compact {
            return format!("STATUS: {label}");
        }
        return String::new();
    }
    format!("STATUS: {text}")
}

#[cfg(test)]
#[path = "tests/top_bar.rs"]
mod tests;
