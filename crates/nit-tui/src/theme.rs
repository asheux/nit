use ratatui::style::Color;
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct ThemeFile {
    background: Option<String>,
    foreground: Option<String>,
    border: Option<String>,
    border_focused: Option<String>,
    title: Option<String>,
    title_focused: Option<String>,
    cursor: Option<String>,
    cursor_line_bg: Option<String>,
    selection_bg: Option<String>,
    warning: Option<String>,
    error: Option<String>,
    accent: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub border: Color,
    pub border_focused: Color,
    pub title: Color,
    pub title_focused: Color,
    pub cursor: Color,
    pub cursor_line_bg: Color,
    pub selection_bg: Color,
    pub warning: Color,
    pub error: Color,
    pub accent: Color,
}

impl Theme {
    pub fn load(path: Option<&Path>) -> Self {
        if let Some(path) = path {
            if let Ok(contents) = fs::read_to_string(path) {
                if let Ok(cfg) = toml::from_str::<ThemeFile>(&contents) {
                    return Theme {
                        background: color_or_default(cfg.background, Color::Rgb(8, 19, 31)),
                        foreground: color_or_default(cfg.foreground, Color::Rgb(215, 229, 255)),
                        border: color_or_default(cfg.border, Color::Rgb(42, 143, 156)),
                        border_focused: color_or_default(
                            cfg.border_focused,
                            Color::Rgb(109, 238, 252),
                        ),
                        title: color_or_default(cfg.title, Color::Rgb(78, 208, 208)),
                        title_focused: color_or_default(
                            cfg.title_focused,
                            Color::Rgb(127, 252, 255),
                        ),
                        cursor: color_or_default(cfg.cursor, Color::Rgb(255, 209, 102)),
                        cursor_line_bg: color_or_default(
                            cfg.cursor_line_bg,
                            Color::Rgb(20, 52, 77),
                        ),
                        selection_bg: color_or_default(cfg.selection_bg, Color::Rgb(27, 63, 92)),
                        warning: color_or_default(cfg.warning, Color::Rgb(242, 165, 65)),
                        error: color_or_default(cfg.error, Color::Rgb(242, 95, 92)),
                        accent: color_or_default(cfg.accent, Color::Rgb(255, 209, 102)),
                    };
                }
            }
        }
        Theme::default()
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            background: Color::Rgb(8, 19, 31),
            foreground: Color::Rgb(215, 229, 255),
            border: Color::Rgb(42, 143, 156),
            border_focused: Color::Rgb(109, 238, 252),
            title: Color::Rgb(78, 208, 208),
            title_focused: Color::Rgb(127, 252, 255),
            cursor: Color::Rgb(255, 209, 102),
            cursor_line_bg: Color::Rgb(20, 52, 77),
            selection_bg: Color::Rgb(27, 63, 92),
            warning: Color::Rgb(242, 165, 65),
            error: Color::Rgb(242, 95, 92),
            accent: Color::Rgb(255, 209, 102),
        }
    }
}

fn color_or_default(value: Option<String>, default: Color) -> Color {
    value
        .as_deref()
        .and_then(parse_hex_color)
        .unwrap_or(default)
}

fn parse_hex_color(s: &str) -> Option<Color> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}
