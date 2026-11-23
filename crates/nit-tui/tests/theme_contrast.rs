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
            theme.hl.doc_comment,
            theme.hl.string,
            theme.hl.number,
            theme.hl.boolean,
            theme.hl.keyword,
            theme.hl.keyword_control,
            theme.hl.r#type,
            theme.hl.function,
            theme.hl.macro_token,
            theme.hl.attribute,
            theme.hl.namespace,
            theme.hl.parameter,
            theme.hl.property,
            theme.hl.link,
            theme.hl.punctuation,
        ] {
            for bg in [
                theme.background,
                theme.panel_background,
                theme.editor_background,
                theme.popup_background,
                theme.cursor_line_bg,
                theme.selection_bg,
            ] {
                let ratio = (luminance(fg) + 0.05) / (luminance(bg) + 0.05);
                assert!(ratio >= 4.5, "{fg:?} on {bg:?} has contrast {ratio:.2}");
            }
        }
    }
}

#[test]
fn default_source_roles_are_distinguishable_in_both_shipped_palettes() {
    let preset = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/themes/devs.toml");
    let defaults = Theme::default();
    let bundled = Theme::load(Some(&preset));
    let roles = |theme: &Theme| {
        [
            theme.hl.keyword,
            theme.hl.function,
            theme.hl.r#type,
            theme.hl.variable,
            theme.hl.string,
            theme.hl.number,
            theme.hl.comment,
            theme.hl.property,
        ]
    };
    let colors = roles(&defaults);
    assert_eq!(colors, roles(&bundled));
    for (i, color) in colors.iter().enumerate() {
        assert!(
            !colors[..i].contains(color),
            "duplicate source color {color:?}"
        );
    }
}

#[test]
fn custom_surfaces_inherit_the_configured_canvas_until_overridden() {
    let path = std::env::temp_dir().join(format!("nit-panel-theme-{}.toml", std::process::id()));
    std::fs::write(&path, "background = '#273849'").unwrap();
    let legacy = Theme::load(Some(&path));
    std::fs::write(
        &path,
        "background = '#273849'\npanel_background = '#364758'\neditor_background = '#293037'",
    )
    .unwrap();
    let custom = Theme::load(Some(&path));
    std::fs::remove_file(path).unwrap();
    assert_eq!(legacy.background, Color::Rgb(39, 56, 73));
    assert_eq!(legacy.panel_background, legacy.background);
    assert_eq!(legacy.editor_background, legacy.background);
    assert_eq!(legacy.popup_background, legacy.background);
    assert_eq!(custom.background, legacy.background);
    assert_eq!(custom.panel_background, Color::Rgb(54, 71, 88));
    assert_eq!(custom.editor_background, Color::Rgb(41, 48, 55));
    assert_eq!(custom.for_popup().background, custom.panel_background);
}
