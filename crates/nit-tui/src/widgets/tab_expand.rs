//! Tab expansion for ratatui `Line` content before rendering.
//!
//! Background: `UnicodeWidthStr::width("\t")` returns 1 (control chars
//! fall through `unwrap_or(1)`), so ratatui's `Paragraph` writes `\t`
//! into a buffer cell unchanged. `crossterm`'s diff loop then `Print`s
//! the tab and the terminal advances the cursor to the next tab stop,
//! while subsequent contiguous cell writes skip `MoveTo` and land past
//! their logical column — leaving the previous frame's chars showing
//! through where the diff believed it had overwritten them. Expanding
//! tabs to spaces before construction sidesteps the whole issue.
//!
//! The expansion is column-aware (next-multiple-of-`TAB_WIDTH` like the
//! editor view) so the visual indentation matches a real tab.
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

const TAB_WIDTH: usize = 4;

pub fn expand_tabs_in_line(line: Line<'static>) -> Line<'static> {
    if !line_contains_tab(&line) {
        return line;
    }
    let Line {
        spans,
        style,
        alignment,
    } = line;
    let mut col = 0usize;
    let mut out_spans: Vec<Span<'static>> = Vec::with_capacity(spans.len());
    for span in spans {
        let (expanded, new_col) = expand_span_content(&span.content, col);
        col = new_col;
        out_spans.push(Span::styled(expanded, span.style));
    }
    let mut new_line = Line::from(out_spans).style(style);
    if let Some(a) = alignment {
        new_line = new_line.alignment(a);
    }
    new_line
}

fn line_contains_tab(line: &Line<'static>) -> bool {
    line.spans.iter().any(|s| s.content.contains('\t'))
}

fn expand_span_content(content: &str, start_col: usize) -> (String, usize) {
    let mut col = start_col;
    let mut out = String::with_capacity(content.len());
    for ch in content.chars() {
        if ch == '\t' {
            let advance = TAB_WIDTH - (col % TAB_WIDTH);
            for _ in 0..advance {
                out.push(' ');
            }
            col += advance;
        } else {
            out.push(ch);
            col += UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        }
    }
    (out, col)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Style};

    #[test]
    fn line_without_tab_is_unchanged() {
        let line = Line::from("hello world");
        let out = expand_tabs_in_line(line.clone());
        assert_eq!(span_strings(&out), vec!["hello world".to_string()]);
    }

    #[test]
    fn single_leading_tab_expands_to_four_spaces() {
        let line = Line::from("\tcode");
        let out = expand_tabs_in_line(line);
        assert_eq!(span_strings(&out).concat(), "    code");
    }

    #[test]
    fn tab_after_text_advances_to_next_stop() {
        // "ab" leaves col=2; next stop is 4 -> 2 spaces.
        let line = Line::from("ab\tcd");
        let out = expand_tabs_in_line(line);
        assert_eq!(span_strings(&out).concat(), "ab  cd");
    }

    #[test]
    fn tab_at_stop_advances_full_width() {
        // "abcd" leaves col=4; next stop is 8 -> 4 spaces.
        let line = Line::from("abcd\tef");
        let out = expand_tabs_in_line(line);
        assert_eq!(span_strings(&out).concat(), "abcd    ef");
    }

    #[test]
    fn col_carries_across_spans() {
        // Two spans: "ab" then "\tx" — the tab in the second span should
        // expand based on col=2, not col=0.
        let spans = vec![
            Span::styled("ab", Style::default().fg(Color::Red)),
            Span::styled("\tx", Style::default().fg(Color::Blue)),
        ];
        let out = expand_tabs_in_line(Line::from(spans));
        assert_eq!(span_strings(&out).concat(), "ab  x");
    }

    #[test]
    fn span_styles_are_preserved() {
        let red = Style::default().fg(Color::Red);
        let blue = Style::default().fg(Color::Blue);
        let spans = vec![Span::styled("ab", red), Span::styled("\tx", blue)];
        let out = expand_tabs_in_line(Line::from(spans));
        let styles: Vec<Style> = out.spans.iter().map(|s| s.style).collect();
        assert_eq!(styles, vec![red, blue]);
    }

    fn span_strings(line: &Line<'static>) -> Vec<String> {
        line.spans.iter().map(|s| s.content.to_string()).collect()
    }
}
