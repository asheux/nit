use crate::theme::Theme;
use nit_core::{Buffer, Mode, PaneId};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

pub struct CursorPlacement {
    pub x: u16,
    pub y: u16,
}

pub fn render_editor(
    frame: &mut Frame,
    area: Rect,
    buffer: &Buffer,
    focus: PaneId,
    _mode: Mode,
    theme: &Theme,
) -> Option<CursorPlacement> {
    render_buffer(
        frame,
        area,
        buffer,
        PaneId::Editor,
        focus,
        "EDITOR  [ SAVE ]",
        theme,
        true,
    )
}

pub fn render_buffer(
    frame: &mut Frame,
    area: Rect,
    buffer: &Buffer,
    pane_id: PaneId,
    focus: PaneId,
    title: &str,
    theme: &Theme,
    show_cursor: bool,
) -> Option<CursorPlacement> {
    let focused = focus == pane_id;
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
        .title(Span::styled(
            title,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ));

    let total_lines = buffer.lines_len().max(1);
    let line_num_width = total_lines.to_string().len().max(3);
    let gutter_width = line_num_width + 4;
    let start = buffer.viewport.offset_line;
    let height = buffer.viewport.height.max(1);
    let end = (start + height).min(total_lines);
    let content_width = buffer.viewport.width.max(1);

    let selection = buffer.selection_range();
    let language = if pane_id == PaneId::Editor {
        detect_language(buffer)
    } else {
        Language::Plain
    };
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for i in start..end {
        let mut content = buffer
            .line_as_string(i)
            .replace('\t', "    ")
            .replace('\r', "");
        if content.ends_with('\n') {
            content.pop();
        }
        let line_len = content.chars().count();
        let offset_col = buffer.viewport.offset_col;
        let is_cursor_line = i == buffer.cursor.line;
        let line_content_width = if is_cursor_line {
            content_width.saturating_sub(1)
        } else {
            content_width
        };
        let visible_start = offset_col.min(line_len);
        let visible_end = (offset_col + line_content_width).min(line_len);
        let ln = format!("{:>width$}", i + 1, width = line_num_width);
        let mut spans = vec![
            Span::styled(
                format!(" {ln} "),
                if is_cursor_line {
                    Style::default()
                        .fg(theme.border_focused)
                        .bg(theme.cursor_line_bg)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                } else {
                    Style::default().fg(theme.border)
                },
            ),
            Span::styled(
                if is_cursor_line { "▌ " } else { "│ " },
                if is_cursor_line {
                    Style::default()
                        .fg(theme.border_focused)
                        .bg(theme.cursor_line_bg)
                        .add_modifier(Modifier::UNDERLINED)
                } else {
                    Style::default().fg(theme.border)
                },
            ),
        ];
        let mut base_style = Style::default().fg(theme.foreground);
        if is_cursor_line {
            base_style = base_style
                .bg(theme.cursor_line_bg)
                .add_modifier(Modifier::UNDERLINED);
        }
        let (chars, mut styles) = styled_line(&content, language, base_style, theme);
        if let Some((start, end)) = selection.and_then(|(start, end)| {
            let line_start = buffer.line_char_start(i);
            let line_end = buffer.line_char_end(i);
            if end <= line_start || start >= line_end {
                return None;
            }
            let mut sel_start = start.saturating_sub(line_start);
            let mut sel_end = end.saturating_sub(line_start);
            if sel_start > line_len {
                sel_start = line_len;
            }
            if sel_end > line_len {
                sel_end = line_len;
            }
            if sel_end <= sel_start {
                None
            } else {
                Some((sel_start, sel_end))
            }
        }) {
            for idx in start..end.min(styles.len()) {
                styles[idx] = styles[idx].bg(theme.selection_bg);
            }
        }
        spans.extend(spans_for_range(
            &chars,
            &styles,
            visible_start,
            visible_end,
            base_style,
        ));
        if line_len == 0 {
            spans.push(Span::styled("", base_style));
        }
        if is_cursor_line {
            spans.push(Span::styled(
                "▐",
                Style::default()
                    .fg(theme.border_focused)
                    .bg(theme.cursor_line_bg)
                    .add_modifier(Modifier::UNDERLINED),
            ));
        }
        lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .block(block);

    frame.render_widget(paragraph, area);

    if show_cursor && focused {
        let cursor_line = buffer.cursor.line.saturating_sub(start);
        let cursor_col = buffer
            .cursor
            .col
            .saturating_sub(buffer.viewport.offset_col);
        let y = area.y + 1 + cursor_line as u16;
        let x = area.x + 1 + gutter_width as u16 + cursor_col as u16;
        return Some(CursorPlacement { x, y });
    }
    None
}

#[derive(Copy, Clone)]
enum Language {
    Rust,
    Toml,
    Json,
    Markdown,
    JavaScript,
    TypeScript,
    Python,
    Go,
    Java,
    C,
    Cpp,
    CSharp,
    Ruby,
    Php,
    Html,
    Css,
    Shell,
    Yaml,
    Sql,
    Lua,
    Kotlin,
    Swift,
    Wolfram,
    Vim,
    Plain,
}

fn detect_language(buffer: &Buffer) -> Language {
    let name = buffer
        .path()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or(buffer.name());
    let lower = name.to_lowercase();
    if lower == "cargo.toml" {
        return Language::Toml;
    }
    if lower == "dockerfile" {
        return Language::Shell;
    }
    if lower == ".vimrc" || lower == "_vimrc" || lower == "vimrc" || lower == ".gvimrc" {
        return Language::Vim;
    }
    let ext = lower.rsplit('.').next().unwrap_or("").to_string();
    match ext.as_str() {
        "rs" => Language::Rust,
        "toml" => Language::Toml,
        "json" => Language::Json,
        "jsonc" => Language::Json,
        "md" | "markdown" => Language::Markdown,
        "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
        "ts" | "tsx" => Language::TypeScript,
        "py" => Language::Python,
        "go" => Language::Go,
        "java" => Language::Java,
        "c" | "h" => Language::C,
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => Language::Cpp,
        "cs" => Language::CSharp,
        "rb" => Language::Ruby,
        "php" => Language::Php,
        "html" | "htm" => Language::Html,
        "css" | "scss" | "sass" => Language::Css,
        "sh" | "bash" | "zsh" | "fish" => Language::Shell,
        "yml" | "yaml" => Language::Yaml,
        "sql" => Language::Sql,
        "lua" => Language::Lua,
        "kt" | "kts" => Language::Kotlin,
        "swift" => Language::Swift,
        "wl" | "wls" | "nb" => Language::Wolfram,
        "vim" => Language::Vim,
        _ => Language::Plain,
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum TokenKind {
    None,
    String,
    Comment,
    Keyword,
    Number,
    Tag,
    Attribute,
}

fn styled_line(line: &str, lang: Language, base: Style, theme: &Theme) -> (Vec<char>, Vec<Style>) {
    let chars: Vec<char> = line.chars().collect();
    let mut styles = vec![base; chars.len()];
    if chars.is_empty() {
        return (chars, styles);
    }

    if matches!(lang, Language::Markdown) {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            for style in &mut styles {
                style.fg = Some(theme.title_focused);
            }
            return (chars, styles);
        }
    }

    let mut kinds = vec![TokenKind::None; chars.len()];
    apply_string_and_comment(&chars, &mut kinds, lang);
    apply_html_css(&chars, &mut kinds, lang);
    apply_keywords_and_numbers(&chars, &mut kinds, lang);

    for (idx, kind) in kinds.iter().enumerate() {
        match kind {
            TokenKind::String => styles[idx].fg = Some(theme.title),
            TokenKind::Comment => styles[idx].fg = Some(theme.border),
            TokenKind::Keyword => styles[idx].fg = Some(theme.accent),
            TokenKind::Number => styles[idx].fg = Some(theme.warning),
            TokenKind::Tag => styles[idx].fg = Some(theme.title_focused),
            TokenKind::Attribute => styles[idx].fg = Some(theme.accent),
            TokenKind::None => {}
        }
    }

    (chars, styles)
}

fn apply_string_and_comment(chars: &[char], kinds: &mut [TokenKind], lang: Language) {
    let delims = string_delims(lang);
    let mut string_delim: Option<char> = None;
    let mut escape = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if string_delim.is_none() {
            if is_comment_start(chars, i, lang) {
                for idx in i..chars.len() {
                    kinds[idx] = TokenKind::Comment;
                }
                break;
            }
        }
        if !delims.is_empty() && delims.contains(&c) && !escape {
            match string_delim {
                Some(d) if d == c => string_delim = None,
                None => string_delim = Some(c),
                _ => {}
            }
            kinds[i] = TokenKind::String;
            i += 1;
            continue;
        }
        if string_delim.is_some() {
            kinds[i] = TokenKind::String;
            if c == '\\' && !escape {
                escape = true;
            } else {
                escape = false;
            }
            i += 1;
            continue;
        }
        escape = false;
        i += 1;
    }
}

fn is_comment_start(chars: &[char], idx: usize, lang: Language) -> bool {
    match lang {
        Language::Rust
        | Language::JavaScript
        | Language::TypeScript
        | Language::Go
        | Language::Java
        | Language::C
        | Language::Cpp
        | Language::CSharp
        | Language::Kotlin
        | Language::Swift
        | Language::Php => {
            (idx + 1 < chars.len() && chars[idx] == '/' && chars[idx + 1] == '/')
                || (idx + 1 < chars.len() && chars[idx] == '/' && chars[idx + 1] == '*')
                || (matches!(lang, Language::Php) && chars[idx] == '#')
        }
        Language::Toml | Language::Yaml | Language::Python | Language::Ruby | Language::Shell => {
            chars[idx] == '#'
        }
        Language::Sql | Language::Lua => {
            idx + 1 < chars.len() && chars[idx] == '-' && chars[idx + 1] == '-'
        }
        Language::Html => {
            idx + 3 < chars.len()
                && chars[idx] == '<'
                && chars[idx + 1] == '!'
                && chars[idx + 2] == '-'
                && chars[idx + 3] == '-'
        }
        Language::Css => {
            idx + 1 < chars.len() && chars[idx] == '/' && chars[idx + 1] == '*'
        }
        Language::Wolfram => {
            idx + 1 < chars.len() && chars[idx] == '(' && chars[idx + 1] == '*'
        }
        Language::Vim => {
            if chars[idx] != '"' {
                return false;
            }
            chars[..idx].iter().all(|c| c.is_whitespace())
        }
        _ => false,
    }
}

fn apply_keywords_and_numbers(chars: &[char], kinds: &mut [TokenKind], lang: Language) {
    if matches!(lang, Language::Markdown | Language::Plain) {
        return;
    }
    let mut i = 0;
    while i < chars.len() {
        if kinds[i] != TokenKind::None {
            i += 1;
            continue;
        }
        let c = chars[i];
        if c.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < chars.len()
                && kinds[i] == TokenKind::None
                && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == '_')
            {
                i += 1;
            }
            for idx in start..i {
                if kinds[idx] == TokenKind::None {
                    kinds[idx] = TokenKind::Number;
                }
            }
            continue;
        }
        if is_word_start(c) {
            let start = i;
            i += 1;
            while i < chars.len() && is_word_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if is_keyword(&word, lang) {
                for idx in start..i {
                    if kinds[idx] == TokenKind::None {
                        kinds[idx] = TokenKind::Keyword;
                    }
                }
            }
            continue;
        }
        i += 1;
    }
}

fn string_delims(lang: Language) -> &'static [char] {
    match lang {
        Language::Json => &['"'],
        Language::Markdown | Language::Plain => &[],
        Language::JavaScript | Language::TypeScript => &['"', '\'', '`'],
        Language::Rust
        | Language::Toml
        | Language::Python
        | Language::Go
        | Language::Java
        | Language::C
        | Language::Cpp
        | Language::CSharp
        | Language::Ruby
        | Language::Php
        | Language::Html
        | Language::Css
        | Language::Shell
        | Language::Yaml
        | Language::Sql
        | Language::Lua
        | Language::Kotlin
        | Language::Swift
        | Language::Wolfram => &['"', '\''],
        Language::Vim => &['\''],
    }
}

fn is_word_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_keyword(word: &str, lang: Language) -> bool {
    match lang {
        Language::Rust => is_rust_keyword(word),
        Language::JavaScript | Language::TypeScript => is_js_keyword(word),
        Language::Python => is_python_keyword(word),
        Language::Go => is_go_keyword(word),
        Language::Java => is_java_keyword(word),
        Language::C => is_c_keyword(word),
        Language::Cpp => is_cpp_keyword(word),
        Language::CSharp => is_csharp_keyword(word),
        Language::Ruby => is_ruby_keyword(word),
        Language::Php => is_php_keyword(word),
        Language::Shell => is_shell_keyword(word),
        Language::Yaml => is_yaml_keyword(word),
        Language::Sql => is_sql_keyword(word),
        Language::Lua => is_lua_keyword(word),
        Language::Kotlin => is_kotlin_keyword(word),
        Language::Swift => is_swift_keyword(word),
        Language::Wolfram => is_wolfram_keyword(word),
        Language::Vim => is_vim_keyword(word),
        Language::Toml => matches!(word, "true" | "false"),
        Language::Json => matches!(word, "true" | "false" | "null"),
        _ => false,
    }
}

fn is_rust_keyword(word: &str) -> bool {
    matches!(
        word,
        "fn"
            | "let"
            | "mut"
            | "struct"
            | "enum"
            | "impl"
            | "trait"
            | "use"
            | "pub"
            | "mod"
            | "match"
            | "if"
            | "else"
            | "while"
            | "for"
            | "loop"
            | "return"
            | "self"
            | "Self"
            | "crate"
            | "super"
            | "const"
            | "static"
            | "async"
            | "await"
            | "move"
            | "ref"
            | "where"
            | "type"
            | "in"
            | "as"
            | "break"
            | "continue"
            | "dyn"
            | "unsafe"
            | "true"
            | "false"
    )
}

fn is_js_keyword(word: &str) -> bool {
    matches!(
        word,
        "function"
            | "const"
            | "let"
            | "var"
            | "class"
            | "extends"
            | "return"
            | "if"
            | "else"
            | "for"
            | "while"
            | "switch"
            | "case"
            | "break"
            | "continue"
            | "import"
            | "export"
            | "from"
            | "new"
            | "this"
            | "try"
            | "catch"
            | "finally"
            | "throw"
            | "async"
            | "await"
            | "yield"
            | "declare"
            | "namespace"
            | "abstract"
            | "readonly"
            | "keyof"
            | "infer"
            | "unknown"
            | "never"
            | "override"
            | "interface"
            | "type"
            | "enum"
            | "implements"
            | "public"
            | "private"
            | "protected"
            | "static"
            | "get"
            | "set"
            | "instanceof"
            | "typeof"
            | "in"
            | "of"
            | "default"
            | "package"
            | "super"
            | "null"
            | "true"
            | "false"
            | "undefined"
    )
}

fn is_python_keyword(word: &str) -> bool {
    matches!(
        word,
        "def"
            | "class"
            | "return"
            | "if"
            | "elif"
            | "else"
            | "for"
            | "while"
            | "try"
            | "except"
            | "finally"
            | "with"
            | "as"
            | "import"
            | "from"
            | "pass"
            | "break"
            | "continue"
            | "lambda"
            | "yield"
            | "global"
            | "nonlocal"
            | "assert"
            | "raise"
            | "True"
            | "False"
            | "None"
            | "and"
            | "or"
            | "not"
            | "in"
            | "is"
    )
}

fn is_go_keyword(word: &str) -> bool {
    matches!(
        word,
        "func"
            | "package"
            | "import"
            | "return"
            | "if"
            | "else"
            | "for"
            | "range"
            | "switch"
            | "case"
            | "break"
            | "continue"
            | "go"
            | "defer"
            | "select"
            | "struct"
            | "interface"
            | "map"
            | "chan"
            | "const"
            | "var"
            | "type"
            | "true"
            | "false"
            | "nil"
    )
}

fn is_java_keyword(word: &str) -> bool {
    matches!(
        word,
        "class"
            | "interface"
            | "enum"
            | "extends"
            | "implements"
            | "public"
            | "private"
            | "protected"
            | "static"
            | "final"
            | "void"
            | "int"
            | "long"
            | "float"
            | "double"
            | "boolean"
            | "char"
            | "byte"
            | "short"
            | "new"
            | "return"
            | "if"
            | "else"
            | "for"
            | "while"
            | "switch"
            | "case"
            | "break"
            | "continue"
            | "try"
            | "catch"
            | "finally"
            | "throw"
            | "throws"
            | "package"
            | "import"
            | "this"
            | "super"
            | "null"
            | "true"
            | "false"
    )
}

fn is_c_keyword(word: &str) -> bool {
    matches!(
        word,
        "int"
            | "char"
            | "float"
            | "double"
            | "void"
            | "if"
            | "else"
            | "for"
            | "while"
            | "switch"
            | "case"
            | "break"
            | "continue"
            | "return"
            | "struct"
            | "typedef"
            | "enum"
            | "const"
            | "static"
            | "extern"
            | "unsigned"
            | "signed"
            | "long"
            | "short"
            | "bool"
            | "true"
            | "false"
    )
}

fn is_cpp_keyword(word: &str) -> bool {
    matches!(
        word,
        "class"
            | "public"
            | "private"
            | "protected"
            | "template"
            | "typename"
            | "namespace"
            | "using"
            | "new"
            | "delete"
            | "this"
            | "virtual"
            | "override"
            | "final"
            | "noexcept"
            | "nullptr"
            | "try"
            | "catch"
            | "throw"
            | "true"
            | "false"
    ) || is_c_keyword(word)
}

fn is_csharp_keyword(word: &str) -> bool {
    matches!(
        word,
        "using"
            | "namespace"
            | "class"
            | "interface"
            | "enum"
            | "struct"
            | "public"
            | "private"
            | "protected"
            | "internal"
            | "static"
            | "readonly"
            | "const"
            | "void"
            | "int"
            | "long"
            | "float"
            | "double"
            | "bool"
            | "string"
            | "new"
            | "return"
            | "if"
            | "else"
            | "for"
            | "while"
            | "switch"
            | "case"
            | "break"
            | "continue"
            | "try"
            | "catch"
            | "finally"
            | "throw"
            | "true"
            | "false"
            | "null"
            | "this"
            | "base"
            | "var"
    )
}

fn is_ruby_keyword(word: &str) -> bool {
    matches!(
        word,
        "def"
            | "class"
            | "module"
            | "end"
            | "if"
            | "elsif"
            | "else"
            | "unless"
            | "while"
            | "until"
            | "for"
            | "in"
            | "do"
            | "begin"
            | "rescue"
            | "ensure"
            | "return"
            | "yield"
            | "self"
            | "true"
            | "false"
            | "nil"
    )
}

fn is_php_keyword(word: &str) -> bool {
    matches!(
        word,
        "function"
            | "class"
            | "public"
            | "private"
            | "protected"
            | "static"
            | "return"
            | "if"
            | "else"
            | "elseif"
            | "for"
            | "foreach"
            | "while"
            | "do"
            | "switch"
            | "case"
            | "break"
            | "continue"
            | "new"
            | "namespace"
            | "use"
            | "trait"
            | "extends"
            | "implements"
            | "true"
            | "false"
            | "null"
    )
}

fn is_shell_keyword(word: &str) -> bool {
    matches!(
        word,
        "if"
            | "then"
            | "fi"
            | "for"
            | "in"
            | "do"
            | "done"
            | "while"
            | "case"
            | "esac"
            | "function"
            | "select"
            | "time"
            | "export"
            | "return"
            | "break"
            | "continue"
    )
}

fn is_yaml_keyword(word: &str) -> bool {
    matches!(word, "true" | "false" | "null")
}

fn is_sql_keyword(word: &str) -> bool {
    let w = word.to_ascii_lowercase();
    matches!(
        w.as_str(),
        "select"
            | "from"
            | "where"
            | "insert"
            | "into"
            | "values"
            | "update"
            | "set"
            | "delete"
            | "create"
            | "table"
            | "alter"
            | "drop"
            | "join"
            | "left"
            | "right"
            | "inner"
            | "outer"
            | "on"
            | "group"
            | "by"
            | "order"
            | "having"
            | "limit"
            | "distinct"
            | "as"
            | "and"
            | "or"
            | "not"
            | "null"
            | "true"
            | "false"
    )
}

fn is_lua_keyword(word: &str) -> bool {
    matches!(
        word,
        "function"
            | "local"
            | "if"
            | "then"
            | "elseif"
            | "else"
            | "end"
            | "for"
            | "while"
            | "repeat"
            | "until"
            | "do"
            | "return"
            | "break"
            | "true"
            | "false"
            | "nil"
    )
}

fn is_kotlin_keyword(word: &str) -> bool {
    matches!(
        word,
        "fun"
            | "val"
            | "var"
            | "class"
            | "object"
            | "interface"
            | "data"
            | "sealed"
            | "when"
            | "if"
            | "else"
            | "for"
            | "while"
            | "return"
            | "null"
            | "true"
            | "false"
            | "is"
            | "in"
            | "as"
            | "this"
            | "super"
            | "package"
            | "import"
    )
}

fn is_swift_keyword(word: &str) -> bool {
    matches!(
        word,
        "func"
            | "let"
            | "var"
            | "class"
            | "struct"
            | "enum"
            | "protocol"
            | "extension"
            | "if"
            | "else"
            | "switch"
            | "case"
            | "for"
            | "while"
            | "return"
            | "import"
            | "public"
            | "private"
            | "internal"
            | "fileprivate"
            | "open"
            | "guard"
            | "defer"
            | "nil"
            | "true"
            | "false"
            | "self"
            | "super"
    )
}

fn is_wolfram_keyword(word: &str) -> bool {
    matches!(
        word,
        "Module"
            | "Block"
            | "With"
            | "Function"
            | "If"
            | "Which"
            | "Do"
            | "For"
            | "While"
            | "Table"
            | "Map"
            | "Select"
            | "Plot"
            | "List"
            | "True"
            | "False"
            | "Null"
            | "Set"
            | "SetDelayed"
            | "Return"
    )
}

fn is_vim_keyword(word: &str) -> bool {
    matches!(
        word,
        "set"
            | "let"
            | "if"
            | "endif"
            | "else"
            | "elseif"
            | "function"
            | "endfunction"
            | "call"
            | "return"
            | "for"
            | "endfor"
            | "while"
            | "endwhile"
            | "map"
            | "noremap"
            | "nnoremap"
            | "inoremap"
            | "vnoremap"
            | "augroup"
            | "autocmd"
            | "command"
            | "syntax"
            | "highlight"
            | "colorscheme"
            | "filetype"
            | "plugin"
            | "finish"
            | "source"
    )
}

fn apply_html_css(chars: &[char], kinds: &mut [TokenKind], lang: Language) {
    match lang {
        Language::Html => highlight_html(chars, kinds),
        Language::Css => highlight_css(chars, kinds),
        _ => {}
    }
}

fn highlight_html(chars: &[char], kinds: &mut [TokenKind]) {
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '<' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        if j < chars.len() && (chars[j] == '!' || chars[j] == '?') {
            i += 1;
            continue;
        }
        if j < chars.len() && chars[j] == '/' {
            j += 1;
        }
        let tag_start = j;
        while j < chars.len() && is_tag_name_char(chars[j]) {
            if kinds[j] == TokenKind::None {
                kinds[j] = TokenKind::Tag;
            }
            j += 1;
        }
        if j == tag_start {
            i += 1;
            continue;
        }
        let mut in_quote: Option<char> = None;
        while j < chars.len() && chars[j] != '>' {
            let c = chars[j];
            if let Some(q) = in_quote {
                if c == q {
                    in_quote = None;
                }
                j += 1;
                continue;
            }
            if c == '"' || c == '\'' {
                in_quote = Some(c);
                j += 1;
                continue;
            }
            if is_attr_start_char(c) {
                let start = j;
                j += 1;
                while j < chars.len() && is_attr_char(chars[j]) {
                    j += 1;
                }
                for idx in start..j {
                    if kinds[idx] == TokenKind::None {
                        kinds[idx] = TokenKind::Attribute;
                    }
                }
                continue;
            }
            j += 1;
        }
        i = j + 1;
    }
}

fn highlight_css(chars: &[char], kinds: &mut [TokenKind]) {
    let mut in_block = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '{' {
            in_block = true;
            i += 1;
            continue;
        }
        if c == '}' {
            in_block = false;
            i += 1;
            continue;
        }
        if !in_block {
            if is_selector_char(c) {
                let start = i;
                i += 1;
                while i < chars.len() && is_selector_char(chars[i]) {
                    i += 1;
                }
                for idx in start..i {
                    if kinds[idx] == TokenKind::None {
                        kinds[idx] = TokenKind::Tag;
                    }
                }
                continue;
            }
        } else if is_attr_start_char(c) {
            let start = i;
            i += 1;
            while i < chars.len() && is_attr_char(chars[i]) {
                i += 1;
            }
            let mut j = i;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && chars[j] == ':' {
                for idx in start..i {
                    if kinds[idx] == TokenKind::None {
                        kinds[idx] = TokenKind::Attribute;
                    }
                }
            }
            continue;
        }
        i += 1;
    }
}

fn is_tag_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == ':'
}

fn is_attr_start_char(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '-'
}

fn is_attr_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':'
}

fn is_selector_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '#' | '-' | '_' | ':')
}

fn spans_for_range(
    chars: &[char],
    styles: &[Style],
    start: usize,
    end: usize,
    base: Style,
) -> Vec<Span<'static>> {
    if chars.is_empty() || start >= end {
        return vec![Span::styled("", base)];
    }
    let mut spans = Vec::new();
    let mut current_style = styles[start];
    let mut buffer = String::new();
    for idx in start..end {
        let style = styles[idx];
        if style != current_style {
            spans.push(Span::styled(buffer.clone(), current_style));
            buffer.clear();
            current_style = style;
        }
        buffer.push(chars[idx]);
    }
    if !buffer.is_empty() {
        spans.push(Span::styled(buffer, current_style));
    }
    spans
}
