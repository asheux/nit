use nit_syntax::HighlightGroup;
use ratatui::style::Color;
use ratatui::style::{Modifier, Style};
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
    success: Option<String>,
    error: Option<String>,
    accent: Option<String>,
    hl: Option<HighlightThemeFile>,
    gol: Option<GolThemeFile>,
    seed: Option<SeedThemeFile>,
}

#[derive(Debug, Deserialize)]
struct HighlightThemeFile {
    comment: Option<String>,
    doc_comment: Option<String>,
    string: Option<String>,
    char: Option<String>,
    number: Option<String>,
    boolean: Option<String>,
    keyword: Option<String>,
    keyword_control: Option<String>,
    keyword_operator: Option<String>,
    r#type: Option<String>,
    type_builtin: Option<String>,
    function: Option<String>,
    method: Option<String>,
    #[serde(rename = "macro")]
    macro_token: Option<String>,
    attribute: Option<String>,
    namespace: Option<String>,
    variable: Option<String>,
    parameter: Option<String>,
    property: Option<String>,
    constant: Option<String>,
    operator: Option<String>,
    punctuation: Option<String>,
    tag: Option<String>,
    heading: Option<String>,
    emphasis: Option<String>,
    link: Option<String>,
    error: Option<String>,
    warning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GolThemeFile {
    bg: Option<String>,
    live_new: Option<String>,
    live: Option<String>,
    live_old: Option<String>,
    trail_1: Option<String>,
    trail_2: Option<String>,
    trail_3: Option<String>,
    bbox: Option<String>,
    hud_dim: Option<String>,
    hud_text: Option<String>,
    hud_spark: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SeedThemeFile {
    bg: Option<String>,
    live: Option<String>,
    live_dim: Option<String>,
    halo_1: Option<String>,
    halo_2: Option<String>,
    grid: Option<String>,
    bbox: Option<String>,
    hud_text: Option<String>,
    hud_dim: Option<String>,
    accent: Option<String>,
    accent_2: Option<String>,
    tissue_palette: Option<Vec<String>>,
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
    pub success: Color,
    pub error: Color,
    pub accent: Color,
    pub hl: HighlightTheme,
    pub gol: GolTheme,
    pub seed: SeedTheme,
}

#[derive(Clone, Debug)]
pub struct GolTheme {
    pub bg: Color,
    pub live_new: Color,
    pub live: Color,
    pub live_old: Color,
    pub trail_1: Color,
    pub trail_2: Color,
    pub trail_3: Color,
    pub bbox: Color,
    pub hud_dim: Color,
    pub hud_text: Color,
    pub hud_spark: Color,
}

#[derive(Clone, Debug)]
pub struct SeedTheme {
    pub bg: Color,
    pub live: Color,
    pub live_dim: Color,
    pub halo_1: Color,
    pub halo_2: Color,
    pub grid: Color,
    pub bbox: Color,
    pub hud_text: Color,
    pub hud_dim: Color,
    pub accent: Color,
    pub accent_2: Color,
    pub tissue_palette: Vec<Color>,
}

#[derive(Clone, Debug)]
pub struct HighlightTheme {
    pub comment: Color,
    pub doc_comment: Color,
    pub string: Color,
    pub char: Color,
    pub number: Color,
    pub boolean: Color,
    pub keyword: Color,
    pub keyword_control: Color,
    pub keyword_operator: Color,
    pub r#type: Color,
    pub type_builtin: Color,
    pub function: Color,
    pub method: Color,
    pub macro_token: Color,
    pub attribute: Color,
    pub namespace: Color,
    pub variable: Color,
    pub parameter: Color,
    pub property: Color,
    pub constant: Color,
    pub operator: Color,
    pub punctuation: Color,
    pub tag: Color,
    pub heading: Color,
    pub emphasis: Color,
    pub link: Color,
    pub error: Color,
    pub warning: Color,
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
                        success: color_or_default(cfg.success, Color::Rgb(154, 216, 143)),
                        error: color_or_default(cfg.error, Color::Rgb(242, 95, 92)),
                        accent: color_or_default(cfg.accent, Color::Rgb(255, 209, 102)),
                        hl: HighlightTheme::from_file(cfg.hl, Color::Rgb(215, 229, 255)),
                        gol: GolTheme::from_file(cfg.gol),
                        seed: SeedTheme::from_file(cfg.seed),
                    };
                }
            }
        }
        Theme::default()
    }

    pub fn highlight_style(&self, group: HighlightGroup) -> Style {
        let color = match group {
            HighlightGroup::Comment => self.hl.comment,
            HighlightGroup::DocComment => self.hl.doc_comment,
            HighlightGroup::String => self.hl.string,
            HighlightGroup::Char => self.hl.char,
            HighlightGroup::Number => self.hl.number,
            HighlightGroup::Boolean => self.hl.boolean,
            HighlightGroup::Keyword => self.hl.keyword,
            HighlightGroup::KeywordControl => self.hl.keyword_control,
            HighlightGroup::KeywordOperator => self.hl.keyword_operator,
            HighlightGroup::Type => self.hl.r#type,
            HighlightGroup::TypeBuiltin => self.hl.type_builtin,
            HighlightGroup::Function => self.hl.function,
            HighlightGroup::Method => self.hl.method,
            HighlightGroup::Macro => self.hl.macro_token,
            HighlightGroup::Attribute => self.hl.attribute,
            HighlightGroup::Namespace => self.hl.namespace,
            HighlightGroup::Variable => self.hl.variable,
            HighlightGroup::Parameter => self.hl.parameter,
            HighlightGroup::Property => self.hl.property,
            HighlightGroup::Constant => self.hl.constant,
            HighlightGroup::Operator => self.hl.operator,
            HighlightGroup::Punctuation => self.hl.punctuation,
            HighlightGroup::Tag => self.hl.tag,
            HighlightGroup::Heading => self.hl.heading,
            HighlightGroup::Emphasis => self.hl.emphasis,
            HighlightGroup::Link => self.hl.link,
            HighlightGroup::Error => self.hl.error,
            HighlightGroup::Warning => self.hl.warning,
            HighlightGroup::DiffAdd | HighlightGroup::DiffRemove | HighlightGroup::Normal => {
                self.foreground
            }
        };
        let mut style = Style::default().fg(color);
        if matches!(group, HighlightGroup::Comment | HighlightGroup::DocComment) {
            style = style.add_modifier(Modifier::DIM);
        }
        if matches!(group, HighlightGroup::Emphasis) {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if matches!(group, HighlightGroup::Heading) {
            style = style.add_modifier(Modifier::BOLD);
        }
        style
    }

    pub fn status_idle_style(&self) -> Style {
        Style::default().fg(self.border).add_modifier(Modifier::DIM)
    }

    pub fn status_ok_style(&self) -> Style {
        Style::default()
            .fg(self.title_focused)
            .add_modifier(Modifier::BOLD)
    }

    pub fn status_warn_style(&self) -> Style {
        Style::default()
            .fg(self.warning)
            .add_modifier(Modifier::BOLD)
    }

    pub fn status_hot_style(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    }

    pub fn status_crit_style(&self) -> Style {
        Style::default()
            .fg(self.background)
            .bg(self.error)
            .add_modifier(Modifier::BOLD)
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
            success: Color::Rgb(154, 216, 143),
            error: Color::Rgb(242, 95, 92),
            accent: Color::Rgb(255, 209, 102),
            hl: HighlightTheme::default(),
            gol: GolTheme::default(),
            seed: SeedTheme::default(),
        }
    }
}

impl GolTheme {
    fn from_file(file: Option<GolThemeFile>) -> Self {
        let Some(file) = file else {
            return GolTheme::default();
        };
        GolTheme {
            bg: color_or_default(file.bg, Color::Rgb(7, 20, 32)),
            live_new: color_or_default(file.live_new, Color::Rgb(168, 255, 247)),
            live: color_or_default(file.live, Color::Rgb(0, 246, 255)),
            live_old: color_or_default(file.live_old, Color::Rgb(0, 179, 192)),
            trail_1: color_or_default(file.trail_1, Color::Rgb(10, 74, 87)),
            trail_2: color_or_default(file.trail_2, Color::Rgb(8, 56, 69)),
            trail_3: color_or_default(file.trail_3, Color::Rgb(6, 39, 52)),
            bbox: color_or_default(file.bbox, Color::Rgb(26, 214, 214)),
            hud_dim: color_or_default(file.hud_dim, Color::Rgb(58, 169, 179)),
            hud_text: color_or_default(file.hud_text, Color::Rgb(127, 252, 255)),
            hud_spark: color_or_default(file.hud_spark, Color::Rgb(255, 209, 102)),
        }
    }
}

impl SeedTheme {
    fn from_file(file: Option<SeedThemeFile>) -> Self {
        let Some(file) = file else {
            return SeedTheme::default();
        };
        let palette = file
            .tissue_palette
            .as_ref()
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| parse_color(value))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        SeedTheme {
            bg: color_or_default(file.bg, Color::Rgb(7, 20, 32)),
            live: color_or_default(file.live, Color::Rgb(0, 246, 255)),
            live_dim: color_or_default(file.live_dim, Color::Rgb(0, 179, 192)),
            halo_1: color_or_default(file.halo_1, Color::Rgb(6, 48, 64)),
            halo_2: color_or_default(file.halo_2, Color::Rgb(8, 69, 90)),
            grid: color_or_default(file.grid, Color::Rgb(11, 47, 61)),
            bbox: color_or_default(file.bbox, Color::Rgb(26, 214, 214)),
            hud_text: color_or_default(file.hud_text, Color::Rgb(127, 252, 255)),
            hud_dim: color_or_default(file.hud_dim, Color::Rgb(58, 169, 179)),
            accent: color_or_default(file.accent, Color::Rgb(255, 209, 102)),
            accent_2: color_or_default(file.accent_2, Color::Rgb(179, 136, 255)),
            tissue_palette: if palette.is_empty() {
                SeedTheme::default().tissue_palette
            } else {
                palette
            },
        }
    }
}

impl Default for SeedTheme {
    fn default() -> Self {
        SeedTheme {
            bg: Color::Rgb(7, 20, 32),
            live: Color::Rgb(0, 246, 255),
            live_dim: Color::Rgb(0, 179, 192),
            halo_1: Color::Rgb(6, 48, 64),
            halo_2: Color::Rgb(8, 69, 90),
            grid: Color::Rgb(11, 47, 61),
            bbox: Color::Rgb(26, 214, 214),
            hud_text: Color::Rgb(127, 252, 255),
            hud_dim: Color::Rgb(58, 169, 179),
            accent: Color::Rgb(255, 209, 102),
            accent_2: Color::Rgb(179, 136, 255),
            tissue_palette: vec![
                Color::Rgb(0, 246, 255),
                Color::Rgb(0, 215, 255),
                Color::Rgb(0, 179, 192),
                Color::Rgb(26, 214, 214),
                Color::Rgb(42, 176, 255),
            ],
        }
    }
}

impl Default for GolTheme {
    fn default() -> Self {
        GolTheme {
            bg: Color::Rgb(7, 20, 32),
            live_new: Color::Rgb(168, 255, 247),
            live: Color::Rgb(0, 246, 255),
            live_old: Color::Rgb(0, 179, 192),
            trail_1: Color::Rgb(10, 74, 87),
            trail_2: Color::Rgb(8, 56, 69),
            trail_3: Color::Rgb(6, 39, 52),
            bbox: Color::Rgb(26, 214, 214),
            hud_dim: Color::Rgb(58, 169, 179),
            hud_text: Color::Rgb(127, 252, 255),
            hud_spark: Color::Rgb(255, 209, 102),
        }
    }
}

impl HighlightTheme {
    fn from_file(file: Option<HighlightThemeFile>, foreground: Color) -> Self {
        let Some(file) = file else {
            return HighlightTheme::default();
        };
        HighlightTheme {
            comment: color_or_default(file.comment, Color::Rgb(84, 117, 150)),
            doc_comment: color_or_default(file.doc_comment, Color::Rgb(122, 193, 255)),
            string: color_or_default(file.string, Color::Rgb(154, 216, 143)),
            char: color_or_default(file.char, Color::Rgb(177, 235, 143)),
            number: color_or_default(file.number, Color::Rgb(242, 165, 65)),
            boolean: color_or_default(file.boolean, Color::Rgb(245, 201, 111)),
            keyword: color_or_default(file.keyword, Color::Rgb(127, 252, 255)),
            keyword_control: color_or_default(file.keyword_control, Color::Rgb(255, 159, 28)),
            keyword_operator: color_or_default(file.keyword_operator, Color::Rgb(255, 209, 102)),
            r#type: color_or_default(file.r#type, Color::Rgb(122, 201, 255)),
            type_builtin: color_or_default(file.type_builtin, Color::Rgb(78, 208, 208)),
            function: color_or_default(file.function, Color::Rgb(255, 209, 102)),
            method: color_or_default(file.method, Color::Rgb(255, 204, 140)),
            macro_token: color_or_default(file.macro_token, Color::Rgb(210, 155, 255)),
            attribute: color_or_default(file.attribute, Color::Rgb(160, 196, 255)),
            namespace: color_or_default(file.namespace, Color::Rgb(109, 238, 252)),
            variable: color_or_default(file.variable, foreground),
            parameter: color_or_default(file.parameter, Color::Rgb(255, 217, 102)),
            property: color_or_default(file.property, Color::Rgb(78, 208, 208)),
            constant: color_or_default(file.constant, Color::Rgb(255, 118, 118)),
            operator: color_or_default(file.operator, Color::Rgb(196, 210, 223)),
            punctuation: color_or_default(file.punctuation, Color::Rgb(108, 147, 177)),
            tag: color_or_default(file.tag, Color::Rgb(78, 208, 208)),
            heading: color_or_default(file.heading, Color::Rgb(127, 252, 255)),
            emphasis: color_or_default(file.emphasis, Color::Rgb(255, 209, 102)),
            link: color_or_default(file.link, Color::Rgb(87, 199, 255)),
            error: color_or_default(file.error, Color::Rgb(242, 95, 92)),
            warning: color_or_default(file.warning, Color::Rgb(242, 165, 65)),
        }
    }
}

impl Default for HighlightTheme {
    fn default() -> Self {
        HighlightTheme {
            comment: Color::Rgb(84, 117, 150),
            doc_comment: Color::Rgb(122, 193, 255),
            string: Color::Rgb(154, 216, 143),
            char: Color::Rgb(177, 235, 143),
            number: Color::Rgb(242, 165, 65),
            boolean: Color::Rgb(245, 201, 111),
            keyword: Color::Rgb(127, 252, 255),
            keyword_control: Color::Rgb(255, 159, 28),
            keyword_operator: Color::Rgb(255, 209, 102),
            r#type: Color::Rgb(122, 201, 255),
            type_builtin: Color::Rgb(78, 208, 208),
            function: Color::Rgb(255, 209, 102),
            method: Color::Rgb(255, 204, 140),
            macro_token: Color::Rgb(210, 155, 255),
            attribute: Color::Rgb(160, 196, 255),
            namespace: Color::Rgb(109, 238, 252),
            variable: Color::Rgb(215, 229, 255),
            parameter: Color::Rgb(255, 217, 102),
            property: Color::Rgb(78, 208, 208),
            constant: Color::Rgb(255, 118, 118),
            operator: Color::Rgb(196, 210, 223),
            punctuation: Color::Rgb(108, 147, 177),
            tag: Color::Rgb(78, 208, 208),
            heading: Color::Rgb(127, 252, 255),
            emphasis: Color::Rgb(255, 209, 102),
            link: Color::Rgb(87, 199, 255),
            error: Color::Rgb(242, 95, 92),
            warning: Color::Rgb(242, 165, 65),
        }
    }
}

fn color_or_default(value: Option<String>, default: Color) -> Color {
    value
        .as_deref()
        .and_then(parse_hex_color)
        .unwrap_or(default)
}

fn parse_color(value: &str) -> Option<Color> {
    parse_hex_color(value)
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
