use super::*;
use crossterm::event::KeyModifiers;
use nit_core::Buffer;
use ratatui::{backend::TestBackend, Terminal};

fn browser() -> AppState {
    let mut state = AppState::new(
        ".".into(),
        Buffer::empty("file", None),
        Buffer::empty("notes", None),
    );
    state.gate_monitor_sub_view = GateMonitorSubView::Skills;
    state.focus = PaneId::GateMonitor;
    state
}

fn draw(state: &mut AppState, area: Rect) -> String {
    let mut term = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    term.draw(|f| render(f, inner(area), state, &Theme::default()))
        .unwrap();
    let buf = term.backend().buffer();
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf.get(x, y).symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn catalog_adapts_and_keeps_the_selected_playbook_visible() {
    let mut state = browser();
    let wide = Rect::new(0, 0, 80, 17);
    let narrow = Rect::new(0, 0, 35, 14);
    let tiny = Rect::new(0, 0, 23, 5);
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let text = draw(&mut state, wide);
    assert!(text.contains(skills::definitions()[0].group) && text.contains("Swarm workflow"));
    assert!(text.contains("nit skills genome-quality"));
    for group in ["Code structure", "Swarm workflow", "Research", "Workspace"] {
        let index = skills::definitions()
            .iter()
            .position(|skill| skill.group == group)
            .unwrap();
        select(&mut state, index);
        let text = draw(&mut state, wide);
        assert!(text.contains(group));
        assert!(text.contains(skills::definitions()[index].name));
    }
    let wide_parts = regions(inner(wide), &state);
    assert!(wide_parts.body.x > wide_parts.catalog.right());
    let narrow_parts = regions(inner(narrow), &state);
    assert!(narrow_parts.body.y >= narrow_parts.catalog.bottom());

    state.focus = PaneId::Editor;
    assert!(!handle_key(&mut state, key(KeyCode::End), wide));
    state.focus = PaneId::GateMonitor;
    assert!(handle_key(&mut state, key(KeyCode::End), wide));
    let last = skills::definitions().len() - 1;
    assert_eq!(state.selected_skill, last);
    for area in [wide, narrow, tiny] {
        let catalog = regions(inner(area), &state).catalog;
        let row = catalog::rows(&catalog::matches(&state))
            .position(|(i, _)| i == Some(last))
            .unwrap()
            - top(&state, catalog, &catalog::matches(&state));
        assert!(row < usize::from(catalog.height));
        assert!(draw(&mut state, area).contains("› "));
    }
    assert!(handle_key(&mut state, key(KeyCode::Enter), tiny));
    assert!(state.skill_reader_open);
    draw(&mut state, narrow);
    assert!(handle_key(&mut state, key(KeyCode::PageDown), narrow));
    assert!(state.gate_monitor_scroll > 0);
    assert!(handle_key(&mut state, key(KeyCode::Esc), narrow));
    assert_eq!(state.selected_skill, last);
    assert!(!state.skill_reader_open);
}

#[test]
fn pointer_selects_scrolls_and_opens_the_displayed_playbook() {
    let mut state = browser();
    let area = Rect::new(0, 0, 76, 15);
    let r = regions(inner(area), &state);
    let second = catalog::rows(&catalog::matches(&state))
        .position(|(i, _)| i == Some(1))
        .unwrap() as u16;
    assert!(click(
        &mut state,
        area,
        Position::new(r.catalog.x, r.catalog.y + second)
    ));
    assert_eq!(state.selected_skill, 1);
    assert!(!state.skill_reader_open);
    let text = draw(&mut state, area);
    assert!(text.contains(skills::definitions()[1].name));
    let body_point = Position::new(r.body.x, r.body.y);
    assert!(wheel(&mut state, area, body_point, 3));
    assert!(state.gate_monitor_scroll > 0);
    assert_eq!(state.selected_skill, 1);
    assert!(wheel(
        &mut state,
        area,
        Position::new(r.catalog.x, r.catalog.y),
        3
    ));
    assert_eq!(state.selected_skill, 2);
    assert_eq!(state.gate_monitor_scroll, 0);
    assert!(click(
        &mut state,
        area,
        Position::new(r.header.x, r.header.y)
    ));
    assert!(state.skill_reader_open);
    let head = regions(inner(area), &state).header;
    assert!(click(&mut state, area, Position::new(head.x, head.y)));
    assert!(!state.skill_reader_open);
}

#[test]
fn filters_by_each_field_and_retains_valid_keyboard_and_mouse_selection() {
    let area = Rect::new(0, 0, 79, 14);
    let mut state = browser();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let type_query = |state: &mut AppState, text: &str| {
        assert!(handle_key(state, key(KeyCode::Char('/')), area));
        for ch in text.chars() {
            assert!(handle_key(state, key(KeyCode::Char(ch)), area));
        }
    };
    state.focus = PaneId::Editor;
    assert!(!handle_key(&mut state, key(KeyCode::Char('/')), area));
    state.focus = PaneId::GateMonitor;
    for (query, id) in [
        ("genome-quality", "genome-quality"),
        ("COMPare encoders", "genome-encoders"),
        ("Research", "research-evidence"),
        ("revalidate assumptions", "workspace-claims"),
    ] {
        type_query(&mut state, query);
        assert!(catalog::matches(&state)
            .iter()
            .any(|&i| skills::definitions()[i].name == id));
        assert!(draw(&mut state, area).contains("found"));
        assert!(handle_key(&mut state, key(KeyCode::Esc), area));
    }
    type_query(&mut state, "Swarm workflow");
    assert!(handle_key(&mut state, key(KeyCode::Enter), area));
    let indices = catalog::matches(&state);
    assert!(handle_key(&mut state, key(KeyCode::End), area));
    assert_eq!(Some(&state.selected_skill), indices.last());
    let r = regions(inner(area), &state);
    assert!(wheel(
        &mut state,
        area,
        Position::new(r.catalog.x, r.catalog.y),
        -3
    ));
    assert_eq!(state.selected_skill, indices[indices.len() - 2]);
    assert!(handle_key(&mut state, key(KeyCode::Enter), area));
    assert!(state.skill_reader_open);
    assert!(handle_key(&mut state, key(KeyCode::Esc), area));
    assert_eq!(state.skill_filter, "Swarm workflow");
    assert!(handle_key(&mut state, key(KeyCode::Esc), area));
    type_query(&mut state, "xyz123");
    assert!(draw(&mut state, area).contains("No matching skills"));
    assert!(lines(&state, &Theme::default(), 30).is_empty());
    assert!(handle_key(&mut state, key(KeyCode::Enter), area));
    assert!(handle_key(&mut state, key(KeyCode::Enter), area));
    assert!(!state.skill_reader_open);
}

#[test]
fn paste_joins_filter_lines_only_while_editing_the_focused_field() {
    let mut state = browser();
    assert!(!paste(&mut state, "initial"));
    state.skill_filter_editing = true;
    state.focus = PaneId::Editor;
    assert!(!paste(&mut state, "editor"));
    state.focus = PaneId::GateMonitor;
    assert!(paste(&mut state, "swarm\r\nworkflow"));
    assert_eq!(state.skill_filter, "swarm workflow");
    assert!(catalog::matches(&state).len() > 1);
}

#[test]
fn multiword_filter_matches_all_terms_across_fields_and_id_separators() {
    let mut state = browser();
    for query in [
        "research experiment",
        "EXPERIMENT research",
        "research-experiment",
        "research_experiment",
        "Research measurements",
    ] {
        state.skill_filter = query.to_owned();
        let ids: Vec<_> = catalog::matches(&state)
            .into_iter()
            .map(|i| skills::definitions()[i].name)
            .collect();
        assert_eq!(ids, ["research-experiment"], "{query}");
    }
    state.skill_filter = "research absentword".into();
    assert!(catalog::matches(&state).is_empty());
}

#[test]
fn catalog_uses_list_background_and_reader_keeps_its_category_color() {
    let mut state = browser();
    let mut theme = Theme::default();
    theme.hl.macro_token = Color::Rgb(201, 155, 235);
    let area = Rect::new(0, 0, 100, 30);
    let mut term = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    let locate = |buf: &ratatui::buffer::Buffer, text: &str| {
        (0..area.height)
            .find_map(|y| {
                let row = (0..area.width)
                    .map(|x| buf.get(x, y).symbol())
                    .collect::<String>();
                row.find(text).map(|i| (row[..i].chars().count() as u16, y))
            })
            .unwrap()
    };
    for (group, color) in [
        ("Code structure", theme.hl.r#type),
        ("Swarm workflow", theme.hl.macro_token),
        ("Research", theme.hl.string),
        ("Workspace", theme.hl.link),
    ] {
        let index = skills::definitions()
            .iter()
            .position(|s| s.group == group)
            .unwrap();
        select(&mut state, index);
        term.draw(|f| render(f, inner(area), &mut state, &theme))
            .unwrap();
        let buf = term.backend().buffer();
        let (x, y) = locate(buf, group);
        assert_eq!(buf.get(x, y).fg, theme.accent);
        assert_eq!(buf.get(x, y).bg, theme.list_background());
        let other = skills::definitions()
            .iter()
            .find(|s| s.name != entry(&state).name)
            .unwrap();
        let (x, y) = locate(buf, other.title);
        assert_eq!(buf.get(x, y).fg, theme.foreground);
        assert_eq!(buf.get(x, y).bg, theme.list_background());
        let (x, y) = locate(buf, "› ");
        assert_eq!(buf.get(x, y).fg, theme.accent);
        assert_eq!(buf.get(x + 2, y).fg, theme.foreground);
        state.skill_reader_open = true;
        term.draw(|f| render(f, inner(area), &mut state, &theme))
            .unwrap();
        let buf = term.backend().buffer();
        let (x, y) = locate(buf, entry(&state).title);
        assert_eq!(buf.get(x, y).fg, color);
        let (x, y) = locate(buf, "‹ Back");
        assert!(buf.get(x, y).modifier.contains(Modifier::UNDERLINED));
        state.skill_reader_open = false;
    }
}

#[test]
fn compact_empty_filter_and_catalog_share_list_surface() {
    let mut state = browser();
    state.skill_filter = "no-such-playbook".into();
    state.skill_filter_editing = true;
    let theme = Theme::default();
    let area = Rect::new(0, 0, 32, 11);
    let r = regions(inner(area), &state);
    let mut term = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    term.draw(|f| render(f, inner(area), &mut state, &theme))
        .unwrap();
    let buf = term.backend().buffer();
    for region in [r.filter, r.catalog] {
        assert!(region.height > 0);
        assert_eq!(
            buf.get(region.right() - 1, region.y).bg,
            theme.list_background()
        );
    }
    assert_eq!(buf.get(r.help.x, r.help.y).bg, theme.list_background());
}

#[test]
fn use_skill_targets_only_the_drawn_label_in_catalog_and_reader() {
    for width in [13, 20, 36, 100] {
        for reader in [false, true] {
            let mut state = browser();
            state.skill_reader_open = reader;
            state.agents.chat_input = "My question".into();
            let area = Rect::new(0, 0, width, 12);
            let r = regions(inner(area), &state);
            let screen = draw(&mut state, area);
            let line = screen.lines().nth(r.help.y as usize).unwrap();
            let start = line.find("Use skill").unwrap() as u16;
            let last = start + "Use skill".len() as u16 - 1;
            assert!(!click(&mut state, area, Position::new(last + 1, r.help.y)));
            assert_eq!(state.agents.chat_input, "My question");
            assert!(click(&mut state, area, Position::new(last, r.help.y)));
            assert_eq!(state.focus, PaneId::Notes);
            assert!(state
                .agents
                .chat_input
                .starts_with("My question\nRead NitSkill "));
            assert!(state.agents.active_turns.is_empty());
        }
    }
}

#[test]
fn filter_allows_mouse_use_while_a_is_text_and_hides_use_for_no_results() {
    let area = Rect::new(0, 0, 55, 16);
    let mut state = browser();
    state.skill_filter_editing = true;
    assert!(handle_key(&mut state, KeyCode::Char('a').into(), area));
    assert_eq!(state.skill_filter, "a");
    assert!(state.agents.chat_input.is_empty());
    let help = regions(inner(area), &state).help;
    let text = draw(&mut state, area);
    let line = text.lines().nth(help.y as usize).unwrap();
    assert!(line.contains("Use skill") && !line.contains("[a]"));
    assert!(click(&mut state, area, Position::new(help.x, help.y)));
    assert_eq!(state.focus, PaneId::Notes);

    let mut state = browser();
    state.skill_filter = "no-matching-playbook".into();
    state.skill_filter_editing = true;
    let help = regions(inner(area), &state).help;
    assert!(!draw(&mut state, area).contains("Use skill"));
    assert!(click(&mut state, area, Position::new(help.x, help.y)));
    assert!(state.skill_filter.is_empty() && state.agents.chat_input.is_empty());
}

#[test]
fn catalog_and_document_share_one_surface_in_every_layout() {
    let theme = Theme {
        background: Color::Rgb(42, 37, 20),
        editor_background: Color::Rgb(87, 72, 51),
        ..Theme::default()
    };
    for width in [30, 100] {
        for (reader, filter) in [(false, ""), (true, ""), (false, "no-such-playbook")] {
            let mut state = browser();
            state.skill_reader_open = reader;
            state.skill_filter = filter.into();
            let area = Rect::new(0, 0, width, 24);
            let mut term = Terminal::new(TestBackend::new(width, 24)).unwrap();
            term.draw(|f| render(f, inner(area), &mut state, &theme))
                .unwrap();
            let buf = term.backend().buffer();
            let content = inner(area);
            for y in content.y..content.bottom() {
                for x in content.x..content.right() {
                    let bg = buf.get(x, y).bg;
                    assert!(
                        bg == theme.list_background() || bg == theme.selection_bg,
                        "{width}×24, x={x}, y={y}, reader={reader}: {bg:?}"
                    );
                }
            }
            assert_ne!(buf.get(0, 0).bg, theme.list_background());
        }
    }
}
