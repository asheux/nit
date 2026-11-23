use super::*;

#[test]
fn catalog_playbooks_fit_the_viewport_and_preserve_inline_content() {
    let theme = Theme::default();
    for (name, _) in nit_core::skills::entries() {
        let body = nit_core::skills::read(name).unwrap();
        for width in [1, 12, 26, 48, 110] {
            let rendered = lines(body, width, &theme, theme.hl.r#type);
            assert!(!rendered.is_empty(), "{name}");
            for line in &rendered {
                assert!(line.width() <= width, "{name} at {width}: {line}");
                let text = line.to_string();
                assert!(
                    !text.starts_with("## ") && !text.contains("**") && !text.contains('`'),
                    "{name}: {text}"
                );
            }
        }
    }
}

#[test]
fn tier_table_retains_columns_on_wide_screens_and_labels_values_on_narrow_ones() {
    let theme = Theme::default();
    let body = nit_core::skills::read("genome-quality").unwrap();
    let wide = lines(body, 110, &theme, theme.hl.r#type);
    assert!(wide.iter().any(|line| {
        let text = line.to_string();
        text.contains("Replicator") && text.contains("2,001 or more")
    }));
    let narrow = lines(body, 30, &theme, theme.hl.r#type);
    assert!(narrow
        .iter()
        .any(|line| line.to_string() == "Label · Replicator"));
}

#[test]
fn headings_lists_and_roles_use_styles_without_markup() {
    let theme = Theme::default();
    let body = "# Reader title\n\n## Overview\n1. Read `commands`.\n- **Check** the result.\nROLE DISCIPLINE: Preserve order.";
    let rendered = lines(body, 28, &theme, theme.hl.r#type);
    let spans: Vec<_> = rendered.iter().flat_map(|r| &r.spans).collect();
    assert_eq!(rendered[0].to_string(), "Overview");
    assert_eq!(rendered[1].to_string(), "1. Read commands.");
    assert_eq!(rendered[2].to_string(), "• Check the result.");
    assert!(spans
        .iter()
        .any(|s| s.content == "commands" && s.style.fg == Some(theme.hl.string)));
    assert!(spans
        .iter()
        .any(|s| s.content == "Check" && s.style.add_modifier.contains(Modifier::BOLD)));
    assert!(rendered.iter().any(|r| r.to_string() == "ROLE DISCIPLINE"));
}

#[test]
fn tables_keep_separators_aligned_through_styled_wide_and_wrapped_cells() {
    use unicode_width::UnicodeWidthStr;
    let theme = Theme::default();
    let body = "| Key | Explanation |\n|---|---|\n| `表` | A long description with several wrapped words retained |\n| **AB** | closing value |";
    let rows = lines(body, 28, &theme, theme.accent);
    let header = rows[0].to_string();
    let sep = header[..header.find('│').unwrap()].width();
    assert_eq!(
        rows[1]
            .to_string()
            .find('┼')
            .map(|i| rows[1].to_string()[..i].width()),
        Some(sep)
    );
    let mut words = Vec::new();
    for row in rows.iter().skip(2) {
        let text = row.to_string();
        assert!(row.width() <= 28);
        let (key, value) = text.split_once('│').unwrap();
        assert_eq!(key.width(), sep);
        words.extend(value.split_whitespace().map(str::to_owned));
    }
    assert_eq!(
        words.join(" "),
        "A long description with several wrapped words retained closing value"
    );
    assert!(rows
        .iter()
        .flat_map(|line| &line.spans)
        .any(|s| s.content == "表" && s.style.fg == Some(theme.hl.string)));
}
