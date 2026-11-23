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
