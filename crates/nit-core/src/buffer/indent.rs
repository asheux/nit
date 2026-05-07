use super::Buffer;

impl Buffer {
    pub(super) fn line_indent(&self, line: usize) -> String {
        if line >= self.rope.len_lines() {
            return String::new();
        }
        let mut indent = String::new();
        for ch in self.rope.line(line).chars() {
            if ch == '\n' {
                break;
            }
            if ch == ' ' || ch == '\t' {
                indent.push(ch);
            } else {
                break;
            }
        }
        indent
    }

    /// Detect the indent unit used in this buffer (e.g. "\t", "  ", "    ").
    pub(super) fn indent_unit(&self) -> String {
        let max = self.rope.len_lines().min(200);
        let mut use_tabs = false;
        let mut widths = Vec::new();
        for i in 0..max {
            let line = self.rope.line(i);
            let mut spaces = 0usize;
            for ch in line.chars() {
                if ch == '\t' {
                    use_tabs = true;
                    break;
                } else if ch == ' ' {
                    spaces += 1;
                } else {
                    break;
                }
            }
            if use_tabs {
                break;
            }
            let has_content = line
                .chars()
                .nth(spaces)
                .is_some_and(|c| c != '\n' && c != '\r');
            if spaces > 0 && has_content {
                widths.push(spaces);
            }
        }
        if use_tabs {
            return "\t".to_string();
        }
        if widths.is_empty() {
            return "    ".to_string();
        }
        let mut g = widths[0];
        for &w in &widths[1..] {
            g = gcd(g, w);
        }
        " ".repeat(g.clamp(1, 8))
    }

    pub(super) fn last_non_ws_char_on_line(&self, line: usize) -> Option<char> {
        if line >= self.rope.len_lines() {
            return None;
        }
        let mut result = None;
        for ch in self.rope.line(line).chars() {
            if ch == '\n' || ch == '\r' {
                break;
            }
            if ch != ' ' && ch != '\t' {
                result = Some(ch);
            }
        }
        result
    }

    pub(super) fn last_non_ws_before_cursor(&self) -> Option<char> {
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let line_start = self.rope.line_to_char(line);
        let idx = self.char_index();
        let mut i = idx;
        while i > line_start {
            let ch = self.rope.char(i - 1);
            if ch != ' ' && ch != '\t' {
                return Some(ch);
            }
            i -= 1;
        }
        None
    }

    pub(super) fn first_non_ws_after_cursor(&self) -> Option<char> {
        let idx = self.char_index();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let line_end_char = self.rope.line_to_char(line) + self.line_char_len(line);
        let mut i = idx;
        while i < line_end_char {
            let ch = self.rope.char(i);
            if ch == '\n' || ch == '\r' {
                return None;
            }
            if ch != ' ' && ch != '\t' {
                return Some(ch);
            }
            i += 1;
        }
        None
    }
}

pub(super) fn is_indent_opener(ch: char) -> bool {
    matches!(ch, '{' | '(' | '[')
}

pub(super) fn matching_closer(opener: char) -> Option<char> {
    match opener {
        '{' => Some('}'),
        '(' => Some(')'),
        '[' => Some(']'),
        _ => None,
    }
}

fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}
