//! Small text-shaping helpers shared across widget renderers.
//!
//! These are kept char-safe (never byte-slicing) so titles, paths, and
//! user-supplied strings containing multi-byte codepoints never panic.

/// Truncate `text` to at most `max_width` chars without adding an ellipsis.
pub(super) fn trim_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    text.chars().take(max_width).collect()
}

/// Truncate `text` to fit within `width` chars, appending `...` when it would
/// overflow. Falls back to a bare char-cut when `width <= 3`.
pub(super) fn truncate_text(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let len = text.chars().count();
    if len <= width {
        return text.to_string();
    }
    if width <= 3 {
        return text.chars().take(width).collect();
    }
    let mut out: String = text.chars().take(width - 3).collect();
    out.push_str("...");
    out
}

/// Truncate `text` to fit within `width` chars, appending the single-char
/// ellipsis `\u{2026}` when overflowing. Used where table rows trust a fixed
/// column width and can't afford the 3-char `...` tail.
pub(super) fn truncate_with_ellipsis(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}
