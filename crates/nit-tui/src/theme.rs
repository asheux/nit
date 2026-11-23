use nit_syntax::HighlightGroup;
use ratatui::style::Color;
use ratatui::style::{Modifier, Style};
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct ThemeFile {
    background: Option<String>,
    panel_background: Option<String>,
    foreground: Option<String>,
    muted: Option<String>,
    border: Option<String>,
    border_focused: Option<String>,
    title: Option<String>,
    title_focused: Option<String>,
    cursor: Option<String>,
    cursor_line_bg: Option<String>,
    selection_bg: Option<String>,
    bracket_match: Option<String>,
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
    pub panel_background: Color,
    pub foreground: Color,
    pub muted: Color,
    pub border: Color,
    pub border_focused: Color,
    pub title: Color,
    pub title_focused: Color,
    pub cursor: Color,
    pub cursor_line_bg: Color,
    pub selection_bg: Color,
    pub bracket_match: Color,
    pub warning: Color,
    pub success: Color,
    pub error: Color,
    pub accent: Color,
    pub hl: HighlightTheme,
    pub gol: GolTheme,
    pub seed: SeedTheme,
    pub diff_added: Color,
    pub diff_modified: Color,
    pub diff_deleted: Color,
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
        let Some(path) = path else {
            return Theme::default();
        };
        let Ok(contents) = fs::read_to_string(path) else {
            return Theme::default();
        };
        let Ok(cfg) = toml::from_str::<ThemeFile>(&contents) else {
            return Theme::default();
        };
        let mut theme = Theme::default();
        overlay(&mut theme.background, cfg.background.as_deref());
        overlay(
            &mut theme.panel_background,
            cfg.panel_background
                .as_deref()
                .or(cfg.background.as_deref()),
        );
        overlay(&mut theme.foreground, cfg.foreground.as_deref());
        overlay(&mut theme.border, cfg.border.as_deref());
        overlay(
            &mut theme.muted,
            cfg.muted.as_deref().or(cfg.border.as_deref()),
        );
        overlay(&mut theme.border_focused, cfg.border_focused.as_deref());
        overlay(&mut theme.title, cfg.title.as_deref());
        overlay(&mut theme.title_focused, cfg.title_focused.as_deref());
        overlay(&mut theme.cursor, cfg.cursor.as_deref());
        overlay(&mut theme.cursor_line_bg, cfg.cursor_line_bg.as_deref());
        overlay(&mut theme.selection_bg, cfg.selection_bg.as_deref());
        overlay(&mut theme.bracket_match, cfg.bracket_match.as_deref());
        overlay(&mut theme.warning, cfg.warning.as_deref());
        overlay(&mut theme.success, cfg.success.as_deref());
        overlay(&mut theme.error, cfg.error.as_deref());
        overlay(&mut theme.accent, cfg.accent.as_deref());
        if let Some(hl) = cfg.hl {
            apply_highlight_overlay(&mut theme.hl, &hl);
        }
        if let Some(gol) = cfg.gol {
            apply_gol_overlay(&mut theme.gol, &gol);
        }
        if let Some(seed) = cfg.seed {
            apply_seed_overlay(&mut theme.seed, &seed);
        }
        theme
    }

    pub fn highlight_style(&self, group: HighlightGroup) -> Style {
        let color = self.highlight_color(group);
        let mut style = Style::default().fg(color);
        if matches!(group, HighlightGroup::Emphasis) {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if matches!(group, HighlightGroup::Heading) {
            style = style.add_modifier(Modifier::BOLD);
        }
        style
    }

    fn highlight_color(&self, group: HighlightGroup) -> Color {
        match group {
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
        }
    }

    pub fn status_idle_style(&self) -> Style {
        Style::default().fg(self.muted)
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
            background: Color::Rgb(10, 21, 30),
            panel_background: Color::Rgb(16, 31, 42),
            foreground: Color::Rgb(222, 234, 239),
            muted: Color::Rgb(171, 191, 199),
            border: Color::Rgb(96, 121, 134),
            border_focused: Color::Rgb(0, 239, 255),
            title: Color::Rgb(171, 191, 199),
            title_focused: Color::Rgb(0, 239, 255),
            cursor: Color::Rgb(0, 239, 255),
            cursor_line_bg: Color::Rgb(18, 49, 61),
            selection_bg: Color::Rgb(22, 67, 82),
            bracket_match: Color::Rgb(39, 83, 102),
            warning: Color::Rgb(255, 195, 99),
            success: Color::Rgb(158, 223, 168),
            error: Color::Rgb(255, 139, 132),
            accent: Color::Rgb(0, 239, 255),
            hl: HighlightTheme::default(),
            gol: GolTheme::default(),
            seed: SeedTheme::default(),
            diff_added: Color::Rgb(158, 223, 168),
            diff_modified: Color::Rgb(122, 219, 228),
            diff_deleted: Color::Rgb(255, 163, 155),
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
            hud_text: Color::Rgb(164, 236, 234),
            hud_spark: Color::Rgb(255, 209, 102),
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
            hud_text: Color::Rgb(164, 236, 234),
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

impl Default for HighlightTheme {
    fn default() -> Self {
        HighlightTheme {
            comment: Color::Rgb(171, 191, 199),
            doc_comment: Color::Rgb(170, 196, 210),
            string: Color::Rgb(158, 223, 168),
            char: Color::Rgb(158, 223, 168),
            number: Color::Rgb(201, 184, 235),
            boolean: Color::Rgb(184, 197, 240),
            keyword: Color::Rgb(184, 197, 240),
            keyword_control: Color::Rgb(184, 197, 240),
            keyword_operator: Color::Rgb(184, 197, 240),
            r#type: Color::Rgb(167, 191, 243),
            type_builtin: Color::Rgb(167, 191, 243),
            function: Color::Rgb(222, 234, 239),
            method: Color::Rgb(222, 234, 239),
            macro_token: Color::Rgb(201, 184, 235),
            attribute: Color::Rgb(170, 196, 210),
            namespace: Color::Rgb(167, 191, 243),
            variable: Color::Rgb(222, 234, 239),
            parameter: Color::Rgb(222, 234, 239),
            property: Color::Rgb(199, 220, 228),
            constant: Color::Rgb(201, 184, 235),
            operator: Color::Rgb(198, 215, 223),
            punctuation: Color::Rgb(171, 191, 199),
            tag: Color::Rgb(167, 191, 243),
            heading: Color::Rgb(222, 234, 239),
            emphasis: Color::Rgb(222, 234, 239),
            link: Color::Rgb(122, 219, 228),
            error: Color::Rgb(255, 139, 132),
            warning: Color::Rgb(255, 195, 99),
        }
    }
}

fn overlay(target: &mut Color, raw: Option<&str>) {
    if let Some(value) = raw.and_then(parse_color) {
        *target = value;
    }
}

fn apply_highlight_overlay(dst: &mut HighlightTheme, src: &HighlightThemeFile) {
    overlay(&mut dst.comment, src.comment.as_deref());
    overlay(&mut dst.doc_comment, src.doc_comment.as_deref());
    overlay(&mut dst.string, src.string.as_deref());
    overlay(&mut dst.char, src.char.as_deref());
    overlay(&mut dst.number, src.number.as_deref());
    overlay(&mut dst.boolean, src.boolean.as_deref());
    overlay(&mut dst.keyword, src.keyword.as_deref());
    overlay(&mut dst.keyword_control, src.keyword_control.as_deref());
    overlay(&mut dst.keyword_operator, src.keyword_operator.as_deref());
    overlay(&mut dst.r#type, src.r#type.as_deref());
    overlay(&mut dst.type_builtin, src.type_builtin.as_deref());
    overlay(&mut dst.function, src.function.as_deref());
    overlay(&mut dst.method, src.method.as_deref());
    overlay(&mut dst.macro_token, src.macro_token.as_deref());
    overlay(&mut dst.attribute, src.attribute.as_deref());
    overlay(&mut dst.namespace, src.namespace.as_deref());
    overlay(&mut dst.variable, src.variable.as_deref());
    overlay(&mut dst.parameter, src.parameter.as_deref());
    overlay(&mut dst.property, src.property.as_deref());
    overlay(&mut dst.constant, src.constant.as_deref());
    overlay(&mut dst.operator, src.operator.as_deref());
    overlay(&mut dst.punctuation, src.punctuation.as_deref());
    overlay(&mut dst.tag, src.tag.as_deref());
    overlay(&mut dst.heading, src.heading.as_deref());
    overlay(&mut dst.emphasis, src.emphasis.as_deref());
    overlay(&mut dst.link, src.link.as_deref());
    overlay(&mut dst.error, src.error.as_deref());
    overlay(&mut dst.warning, src.warning.as_deref());
}

fn apply_gol_overlay(dst: &mut GolTheme, src: &GolThemeFile) {
    overlay(&mut dst.bg, src.bg.as_deref());
    overlay(&mut dst.live_new, src.live_new.as_deref());
    overlay(&mut dst.live, src.live.as_deref());
    overlay(&mut dst.live_old, src.live_old.as_deref());
    overlay(&mut dst.trail_1, src.trail_1.as_deref());
    overlay(&mut dst.trail_2, src.trail_2.as_deref());
    overlay(&mut dst.trail_3, src.trail_3.as_deref());
    overlay(&mut dst.bbox, src.bbox.as_deref());
    overlay(&mut dst.hud_dim, src.hud_dim.as_deref());
    overlay(&mut dst.hud_text, src.hud_text.as_deref());
    overlay(&mut dst.hud_spark, src.hud_spark.as_deref());
}

fn apply_seed_overlay(dst: &mut SeedTheme, src: &SeedThemeFile) {
    overlay(&mut dst.bg, src.bg.as_deref());
    overlay(&mut dst.live, src.live.as_deref());
    overlay(&mut dst.live_dim, src.live_dim.as_deref());
    overlay(&mut dst.halo_1, src.halo_1.as_deref());
    overlay(&mut dst.halo_2, src.halo_2.as_deref());
    overlay(&mut dst.grid, src.grid.as_deref());
    overlay(&mut dst.bbox, src.bbox.as_deref());
    overlay(&mut dst.hud_text, src.hud_text.as_deref());
    overlay(&mut dst.hud_dim, src.hud_dim.as_deref());
    overlay(&mut dst.accent, src.accent.as_deref());
    overlay(&mut dst.accent_2, src.accent_2.as_deref());
    if let Some(raw) = src.tissue_palette.as_ref() {
        let palette: Vec<Color> = raw.iter().filter_map(|s| parse_color(s)).collect();
        if !palette.is_empty() {
            dst.tissue_palette = palette;
        }
    }
}

// Accepts `#RRGGBB` and `RRGGBB`. Any other form (shorthand, alpha, named
// colors) is rejected — palettes are explicit so mistakes are obvious.
fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}
