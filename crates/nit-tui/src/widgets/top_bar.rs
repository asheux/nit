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
use crate::vitals::{sparkline_from_samples, LabCriticality, LabVitalsSnapshot};

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
    let file_text = file.unwrap_or_else(|| state.editor_buffer().name().to_string());
    let status_text = state.status.as_deref().unwrap_or_default();
    let compact_status_mode =
        matches!(state.app_kind, nit_core::AppKind::Games) && state.games.running;
    let status_label = build_status_label(status_text, compact_status_mode);

    let inner_width = area.width.saturating_sub(2) as usize;
    let fixed_width = [" nit ", " | ", " | ", &mode, " | ", app_label, " | UTF-8 "]
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
        format!("{}*", truncate_start(&file_text, name_max))
    };

    let mut spans = vec![
        Span::styled(
            " nit ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | ", Style::default().fg(theme.border)),
        Span::styled(
            file_display,
            Style::default()
                .fg(if dirty.is_empty() {
                    theme.foreground
                } else {
                    theme.warning
                })
                .add_modifier(if dirty.is_empty() {
                    Modifier::empty()
                } else {
                    Modifier::BOLD
                }),
        ),
        Span::styled(" | ", Style::default().fg(theme.border)),
        Span::styled(mode, Style::default().fg(theme.accent)),
        Span::styled(" | ", Style::default().fg(theme.border)),
        Span::styled(app_label, Style::default().fg(theme.accent)),
        Span::styled(" | UTF-8 ", Style::default().fg(theme.border)),
    ];

    let left_width: usize = spans.iter().map(|s| s.content.as_ref().width()).sum();
    let max_right = inner_width.saturating_sub(left_width).saturating_sub(1);
    let (mut vitals_spans, mut right_width) = build_vitals_spans(vitals, theme, max_right);
    if !status_label.is_empty() && max_right > right_width + 4 {
        let status_max = max_right.saturating_sub(right_width + 2);
        let trimmed = truncate_start(&status_label, status_max);
        if !trimmed.is_empty() {
            right_width += 2 + trimmed.width();
            vitals_spans.push(Span::raw("  "));
            vitals_spans.push(Span::styled(trimmed, status_style(status_text, theme)));
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
    let label_style = theme.status_idle_style();
    let value_style = Style::default().fg(theme.foreground);
    let hb_style = if vitals.job_running {
        value_style
    } else {
        theme.status_idle_style()
    };
    let ag_style = if vitals.agent_enabled && vitals.agent_connected {
        value_style
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
        let ecg = severity_scaled_waveform(vitals, ecg_width);
        let mut spans = vec![
            Span::styled("LAB ", label_style),
            Span::styled(level.clone(), level_style),
            Span::styled("  ECG ", label_style),
            Span::styled(ecg, level_style),
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

fn severity_scaled_waveform(vitals: &LabVitalsSnapshot, width: usize) -> String {
    let scaled = severity_scaled_samples(&vitals.ecg_samples, vitals.criticality);
    sparkline_from_samples(&scaled, width)
}

fn severity_scaled_samples(samples: &[u64], level: LabCriticality) -> Vec<u64> {
    let (floor, scale) = match level {
        LabCriticality::Idle | LabCriticality::Ok => (0u64, 1.0f64),
        LabCriticality::Warn => (16u64, 1.15f64),
        LabCriticality::Hot => (30u64, 1.35f64),
        LabCriticality::Crit => (45u64, 1.60f64),
    };
    samples
        .iter()
        .map(|sample| {
            let amplified = ((*sample as f64) * scale).round() as u64;
            amplified.max(floor).min(100)
        })
        .collect()
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
    } else if lower.contains("queued")
        || lower.contains("running")
        || lower.contains("loading")
        || lower.contains("pending")
    {
        theme.warning
    } else {
        theme.accent
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn build_status_label(status_text: &str, compact_mode: bool) -> String {
    let text = status_text.trim();
    if text.is_empty() {
        return String::new();
    }
    if compact_mode {
        if is_high_priority_status(text) {
            return format!("STATUS: {text}");
        }
        let lower = text.to_ascii_lowercase();
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

fn is_high_priority_status(status_text: &str) -> bool {
    let lower = status_text.to_ascii_lowercase();
    lower.contains("error")
        || lower.contains("failed")
        || lower.contains("panic")
        || lower.contains("fatal")
        || lower.contains("warn")
        || lower.contains("timeout")
        || lower.contains("crash")
}

#[cfg(test)]
mod tests {
    use super::{build_status_label, severity_scaled_samples};
    use crate::vitals::LabCriticality;

    #[test]
    fn status_label_non_compact_keeps_full_message() {
        assert_eq!(
            build_status_label("Games tournament queued", false),
            "STATUS: Games tournament queued"
        );
    }

    #[test]
    fn status_label_compact_maps_routine_states() {
        assert_eq!(
            build_status_label("Games tournament queued", true),
            "STATUS: QUEUED"
        );
        assert_eq!(
            build_status_label("Games analysis started", true),
            "STATUS: BUSY"
        );
        assert_eq!(
            build_status_label("Games tournament completed", true),
            "STATUS: DONE"
        );
    }

    #[test]
    fn status_label_compact_keeps_high_priority_detail() {
        assert_eq!(
            build_status_label("Games tournament failed: timeout", true),
            "STATUS: Games tournament failed: timeout"
        );
    }

    #[test]
    fn severity_scaling_increases_warn_hot_crit_bar_size() {
        let base = vec![10, 20, 30, 40, 50];
        let warn = severity_scaled_samples(&base, LabCriticality::Warn);
        let hot = severity_scaled_samples(&base, LabCriticality::Hot);
        let crit = severity_scaled_samples(&base, LabCriticality::Crit);

        assert!(warn.iter().zip(base.iter()).all(|(w, b)| *w >= *b));
        assert!(hot.iter().zip(warn.iter()).all(|(h, w)| *h >= *w));
        assert!(crit.iter().zip(hot.iter()).all(|(c, h)| *c >= *h));
    }
}
