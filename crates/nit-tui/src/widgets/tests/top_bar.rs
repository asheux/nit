use std::time::Duration;

use super::{
    build_status_label, build_vitals_spans, criticality_style, minify_status_text, status_style,
};
use crate::vitals::severity_scaled_samples;
use crate::vitals::{LabCriticality, LabVitalsSnapshot};
use crate::Theme;

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
fn status_label_compact_sanitizes_error_detail() {
    assert_eq!(
        build_status_label("Games tournament failed: timeout", true),
        "STATUS: ERROR"
    );
}

#[test]
fn minify_status_text_strips_opened_path() {
    assert_eq!(minify_status_text("Opened /tmp/foo.txt"), "Opened");
}

#[test]
fn minify_status_text_keeps_short_colon_values() {
    assert_eq!(minify_status_text("Heat: ON"), "Heat: ON");
}

#[test]
fn minify_status_text_drops_long_colon_payloads() {
    assert_eq!(
        minify_status_text("Save failed: permission denied (os error 13)"),
        "Save failed"
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

#[test]
fn status_style_busy_avoids_warning_and_accent_yellow() {
    let theme = Theme::default();
    let style = status_style("STATUS: BUSY", &theme);
    assert_ne!(style.fg, Some(theme.warning));
    assert_ne!(style.fg, Some(theme.accent));
    assert_ne!(style.fg, Some(theme.border_focused));
    assert_ne!(style.fg, Some(theme.title));
    assert_ne!(style.fg, Some(theme.title_focused));
    assert_eq!(style.fg, Some(theme.foreground));
}

#[test]
fn criticality_styles_are_visually_distinct() {
    let theme = Theme::default();
    let ok = criticality_style(LabCriticality::Ok, &theme);
    let warn = criticality_style(LabCriticality::Warn, &theme);
    let hot = criticality_style(LabCriticality::Hot, &theme);
    let crit = criticality_style(LabCriticality::Crit, &theme);

    assert_ne!(ok, warn);
    assert_ne!(warn, hot);
    assert_ne!(hot, crit);
    assert_eq!(warn.fg, Some(theme.warning));
    assert_eq!(hot.fg, Some(theme.accent));
    assert_eq!(crit.bg, Some(theme.error));
}

#[test]
fn crit_ecg_waveform_avoids_reverse_background_style() {
    let theme = Theme::default();
    let vitals = LabVitalsSnapshot {
        criticality: LabCriticality::Crit,
        hb_age: Some(Duration::from_secs(12)),
        ag_age: Some(Duration::from_secs(2)),
        job_running: true,
        agent_enabled: true,
        agent_connected: true,
        ecg_samples: vec![0, 20, 55, 80, 40, 95, 10],
    };
    let (spans, _) = build_vitals_spans(&vitals, &theme, 80);

    // spans = ["LAB ", level, "  ECG ", waveform, ...]
    assert!(
        spans.len() >= 4,
        "expected ECG waveform span to be present, got {spans:?}"
    );
    assert_eq!(
        spans[3].style.bg, None,
        "ECG waveform should not be reverse-filled"
    );
    assert_eq!(spans[3].style.fg, Some(theme.error));
}
