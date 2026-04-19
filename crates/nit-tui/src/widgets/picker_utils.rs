//! Layout and text helpers shared by the popup picker widgets
//! (`protocol_picker`, `rule_picker`). Kept `pub(crate)` so internal pickers
//! can reuse them without expanding the crate's public surface.

use ratatui::layout::Rect;

const ELLIPSIS: &str = "...";

/// Char-safe truncation with an ellipsized `...` suffix once the text exceeds
/// `max` and there is room for the suffix. Shared by `protocol_picker` and
/// `rule_picker`; both render Unicode rule names whose byte counts diverge from
/// char counts, so the naive `&str[..max]` cut would slice mid-codepoint.
pub(crate) fn truncate_text(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.chars().count() > max && max > ELLIPSIS.len() {
        let mut out: String = text.chars().take(max - ELLIPSIS.len()).collect();
        out.push_str(ELLIPSIS);
        out
    } else {
        text.chars().take(max).collect()
    }
}

/// Word-wrap `text` into lines of at most `width` display chars. Words longer
/// than `width` are hard-broken on char boundaries so no output row overflows
/// the picker column. An empty trimmed input yields a single empty line so the
/// caller can still render an anchor row.
pub(crate) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || width == 0 {
        return vec![String::new()];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in trimmed.split_whitespace() {
        let word_len = word.chars().count();
        if word_len > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            let mut chunk = String::new();
            for ch in word.chars() {
                chunk.push(ch);
                if chunk.chars().count() == width {
                    lines.push(std::mem::take(&mut chunk));
                }
            }
            if !chunk.is_empty() {
                current = chunk;
            }
            continue;
        }
        let needs_space = !current.is_empty();
        let next_len = current.chars().count() + usize::from(needs_space) + word_len;
        if next_len <= width {
            if needs_space {
                current.push(' ');
            }
            current.push_str(word);
        } else {
            lines.push(std::mem::replace(&mut current, word.to_string()));
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Center a `width x height` sub-rect inside `screen`, clamping to screen size.
pub(crate) fn centered_rect_px(screen: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(screen.width);
    let h = height.min(screen.height);
    Rect {
        x: screen.x + screen.width.saturating_sub(w) / 2,
        y: screen.y + screen.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}
