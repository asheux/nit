//! Layout and text helpers shared by the popup picker widgets.

use ratatui::layout::Rect;

const ELLIPSIS: &str = "...";

// Char-safe so Unicode rule names (byte count ≠ char count) never slice mid-codepoint.
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

// Words longer than `width` are hard-broken on char boundaries so no row
// overflows. Empty input still yields one empty line for anchor rendering.
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
