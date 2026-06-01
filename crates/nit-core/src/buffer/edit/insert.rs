use super::super::indent::{is_block_starter, matching_closer};
use super::super::undo_log::GroupHint;
use super::super::Buffer;

impl Buffer {
    pub(in crate::buffer) fn replace_selection_with_str(&mut self, s: &str) {
        let Some((start, end)) = self.selection_range() else {
            return;
        };
        if start >= end {
            self.clear_selection();
            return;
        }

        self.end_edit_group();
        self.begin_undo_group();
        let removed = self.rope.slice(start..end).to_string();
        let pre_remove_cursor = self.cursor;
        self.apply_selection_removal(start, end);
        self.record_delete_delta(start, &removed, pre_remove_cursor, GroupHint::Explicit);

        if !s.is_empty() {
            let idx = self.char_index();
            let before = self.cursor;
            self.record_insert(idx, s);
            self.rope.insert(idx, s);
            self.advance_cursor_through(s);
            self.dirty = true;
            self.record_insert_delta(idx, s, before, GroupHint::Explicit);
        } else {
            self.dirty = true;
        }
        self.end_undo_group();
    }

    pub(in crate::buffer) fn replace_selection_with_newline_preserve_indent(&mut self) {
        let Some((start, end)) = self.selection_range() else {
            return;
        };
        if start >= end {
            self.clear_selection();
            return;
        }

        self.end_edit_group();
        self.begin_undo_group();
        let removed = self.rope.slice(start..end).to_string();
        let pre_remove_cursor = self.cursor;
        self.apply_selection_removal(start, end);
        self.record_delete_delta(start, &removed, pre_remove_cursor, GroupHint::Explicit);

        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let indent = self.line_indent(line);
        let idx = self.char_index();
        let (text, cursor_col) = self.build_indented_newline(&indent);

        let before = self.cursor;
        self.record_insert(idx, &text);
        self.rope.insert(idx, &text);
        self.cursor.line += 1;
        self.cursor.col = cursor_col;
        self.dirty = true;
        self.record_insert_delta(idx, &text, before, GroupHint::Explicit);
        self.end_undo_group();
    }

    pub fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.selection_range().is_some() {
            self.replace_selection_with_str(s);
            return;
        }
        let idx = self.char_index();
        let before = self.cursor;
        self.record_insert(idx, s);
        self.rope.insert(idx, s);
        self.advance_cursor_through(s);
        self.dirty = true;
        let hint = Self::classify_insert_hint(s);
        self.record_insert_delta(idx, s, before, hint);
    }

    pub fn insert_char(&mut self, c: char) {
        if self.selection_range().is_some() {
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf);
            self.replace_selection_with_str(s);
            return;
        }
        let idx = self.char_index();
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        let before = self.cursor;
        self.record_insert(idx, s);
        self.rope.insert_char(idx, c);
        self.cursor.col += 1;
        self.dirty = true;
        let hint = Self::classify_insert_hint(s);
        self.record_insert_delta(idx, s, before, hint);
    }

    /// Auto-pair: insert opener+closer as one atomic transaction so a single
    /// undo rewinds both characters.
    pub fn insert_pair(&mut self, open: char, close: char) {
        if self.selection_range().is_some() {
            let mut buf = [0u8; 4];
            let s = open.encode_utf8(&mut buf);
            self.replace_selection_with_str(s);
            return;
        }
        self.end_edit_group();
        let idx = self.char_index();
        let mut s = String::with_capacity(open.len_utf8() + close.len_utf8());
        s.push(open);
        s.push(close);
        let before = self.cursor;
        self.record_insert(idx, &s);
        self.rope.insert(idx, &s);
        self.cursor.col += 1;
        self.dirty = true;
        self.record_insert_delta(idx, &s, before, GroupHint::Atomic);
    }

    pub fn insert_tab(&mut self) {
        self.insert_char('\t');
    }

    /// Insert mode `<CR>`. Each newline is its own atomic transaction. The
    /// generated text — newline plus indent (plus optional partner line when
    /// the cursor sits between an open/close bracket pair) — is recorded as
    /// one insert delta so a single undo collapses it.
    pub fn insert_newline(&mut self) {
        if self.selection_range().is_some() {
            self.replace_selection_with_newline_preserve_indent();
            return;
        }
        self.end_edit_group();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let indent = self.line_indent(line);
        let idx = self.char_index();
        let (text, cursor_col) = self.build_indented_newline(&indent);

        let before = self.cursor;
        self.record_insert(idx, &text);
        self.rope.insert(idx, &text);
        self.cursor.line += 1;
        self.cursor.col = cursor_col;
        self.dirty = true;
        self.record_insert_delta(idx, &text, before, GroupHint::Atomic);
    }

    pub fn open_line_below(&mut self) {
        self.end_edit_group();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let indent = self.line_indent(line);

        let last_char = self.last_non_ws_char_on_line(line);
        let language = self.language_label();
        let extra_indent = if last_char.is_some_and(|c| is_block_starter(c, language)) {
            self.indent_unit()
        } else {
            String::new()
        };

        let insert_at = self.rope.line_to_char(line) + self.line_char_len(line);
        let mut text = String::from("\n");
        text.push_str(&indent);
        text.push_str(&extra_indent);
        let before = self.cursor;
        self.record_insert(insert_at, &text);
        self.rope.insert(insert_at, &text);
        self.cursor.line = line + 1;
        self.cursor.col = indent.chars().count() + extra_indent.chars().count();
        self.dirty = true;
        self.record_insert_delta(insert_at, &text, before, GroupHint::Atomic);
    }

    pub fn open_line_above(&mut self) {
        self.end_edit_group();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let indent = if line > 0 {
            self.line_indent(line.saturating_sub(1))
        } else {
            String::new()
        };
        let idx = self.rope.line_to_char(line);
        let mut text = indent.clone();
        text.push('\n');
        let before = self.cursor;
        self.record_insert(idx, &text);
        self.rope.insert(idx, &text);
        self.cursor.line = line;
        self.cursor.col = indent.chars().count();
        self.dirty = true;
        self.record_insert_delta(idx, &text, before, GroupHint::Atomic);
    }

    pub fn paste_line_above(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.end_edit_group();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let idx = self.rope.line_to_char(line);
        // Linewise paste opens a fresh line: terminate the payload so its last
        // line can't fuse into the line it pushes down.
        let mut insert_text = text.to_string();
        if !insert_text.ends_with('\n') {
            insert_text.push('\n');
        }
        let before = self.cursor;
        self.record_insert(idx, &insert_text);
        self.rope.insert(idx, &insert_text);
        self.cursor.line = line;
        self.cursor.col = 0;
        self.dirty = true;
        self.clamp_col();
        self.record_insert_delta(idx, &insert_text, before, GroupHint::Atomic);
    }

    pub fn paste_line_below(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.end_edit_group();
        let total = self.rope.len_lines();
        let line = self.cursor.line.min(total.saturating_sub(1));
        let idx = if line + 1 < total {
            self.rope.line_to_char(line + 1)
        } else {
            self.rope.len_chars()
        };
        let mut insert_text = String::new();
        if idx > 0 && self.rope.char(idx.saturating_sub(1)) != '\n' {
            insert_text.push('\n');
        }
        insert_text.push_str(text);
        // Terminate the payload so the block's last line lands on its own row
        // instead of fusing into the existing next line (the `}    args = …`
        // collision when a visual `y` slice ends mid-line).
        if !insert_text.ends_with('\n') {
            insert_text.push('\n');
        }
        let before = self.cursor;
        self.record_insert(idx, &insert_text);
        self.rope.insert(idx, &insert_text);
        self.cursor.line = (line + 1).min(self.rope.len_lines().saturating_sub(1));
        self.cursor.col = 0;
        self.dirty = true;
        self.clamp_col();
        self.record_insert_delta(idx, &insert_text, before, GroupHint::Atomic);
    }

    fn advance_cursor_through(&mut self, s: &str) {
        let mut line = self.cursor.line;
        let mut col = self.cursor.col;
        for ch in s.chars() {
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        self.cursor.line = line;
        self.cursor.col = col;
    }

    /// Compute the post-newline insertion text and resulting cursor column for
    /// an indent-aware newline at the current cursor position.
    /// Returns `(text_to_insert, cursor_col_after)`.
    //
    // The increase predicate is language-aware via `is_block_starter`:
    // brackets always trigger; Python's `:` only triggers when the buffer
    // is `.py`. Bracket-pair expansion (the `fn foo(|)` → three-line case)
    // is gated on `matching_closer`, which returns `None` for `:`, so a
    // Python `:` correctly indents the next line without inserting a
    // synthetic partner.
    //
    // Terminator dedent (T8a): when the line being left is a control-flow
    // terminator (`return` / `break` / `continue`, plus Python `pass` /
    // `raise`) and no bracket-opener pushes the indent in the other
    // direction, drop one indent unit from the inherited indent — the
    // surrounding block has logically ended.
    fn build_indented_newline(&self, indent: &str) -> (String, usize) {
        let char_before = self.last_non_ws_before_cursor();
        let language = self.language_label();
        let should_increase = char_before.is_some_and(|c| is_block_starter(c, language));
        let char_after = self.first_non_ws_after_cursor();
        let bracket_pair = should_increase
            && char_before
                .and_then(matching_closer)
                .zip(char_after)
                .is_some_and(|(expected, actual)| expected == actual);

        let extra_indent = if should_increase {
            self.indent_unit()
        } else {
            String::new()
        };

        let leading_indent = if !should_increase && self.line_prefix_ends_with_terminator(language)
        {
            dedent_by_one(indent, &self.indent_unit())
        } else {
            indent.to_string()
        };

        let mut text = String::from("\n");
        text.push_str(&leading_indent);
        text.push_str(&extra_indent);
        if bracket_pair {
            text.push('\n');
            text.push_str(indent);
        }
        let cursor_col = leading_indent.chars().count() + extra_indent.chars().count();
        (text, cursor_col)
    }
}

/// Strip one `unit` of trailing whitespace from `indent`, leaving the rest
/// untouched. Returns the original string when `indent` doesn't end with
/// `unit`, so a flush-left line stays flush-left rather than panicking.
fn dedent_by_one(indent: &str, unit: &str) -> String {
    indent
        .strip_suffix(unit)
        .map(str::to_owned)
        .unwrap_or_else(|| indent.to_owned())
}
