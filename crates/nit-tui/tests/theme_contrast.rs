use nit_tui::theme::Theme;
use ratatui::style::Color;
use std::path::Path;

fn luminance(color: Color) -> f64 {
    let Color::Rgb(r, g, b) = color else {
        panic!("bundled palettes must specify RGB colors");
    };
    let linear = |byte: u8| {
        let value = f64::from(byte) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

#[test]
fn shipped_text_colors_keep_contrast_on_normal_and_selected_rows() {
    let preset = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/themes/devs.toml");
    assert!(preset.is_file());
    for theme in [Theme::default(), Theme::load(Some(&preset))] {
        for fg in [
            theme.foreground,
            theme.muted,
            theme.title,
            theme.title_focused,
            theme.accent,
            theme.warning,
            theme.success,
            theme.error,
            theme.hl.comment,
            theme.hl.punctuation,
        ] {
            for bg in [theme.background, theme.cursor_line_bg, theme.selection_bg] {
                let ratio = (luminance(fg) + 0.05) / (luminance(bg) + 0.05);
                assert!(ratio >= 4.5, "{fg:?} on {bg:?} has contrast {ratio:.2}");
            }
        }
    }
}
