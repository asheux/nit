use super::*;
use nit_core::Buffer;
use ratatui::{backend::TestBackend, Terminal};

#[test]
fn selected_row_spans_the_tree_interior_and_retains_nested_alignment() {
    let mut state = AppState::new(
        ".".into(),
        Buffer::empty("e", None),
        Buffer::empty("n", None),
    );
    state.file_tree.rows = [
        ("▾ src/", FileTreeKind::Dir, 0),
        ("    lib.rs", FileTreeKind::File, 1),
        ("    tests.rs", FileTreeKind::File, 1),
    ]
    .into_iter()
    .map(|(text, kind, depth)| FileTreeRow {
        text: text.into(),
        path: text.into(),
        kind,
        depth,
    })
    .collect();
    state.file_tree.selected = 1;
    let theme = Theme::default();
    for width in [18, 65] {
        for focused in [true, false] {
            state.focus = if focused {
                PaneId::Editor
            } else {
                PaneId::Notes
            };
            let mut terminal = Terminal::new(TestBackend::new(width, 7)).unwrap();
            terminal
                .draw(|frame| render(frame, frame.size(), &state, &theme))
                .unwrap();
            let buf = terminal.backend().buffer();
            let bg = if focused {
                theme.selection_bg
            } else {
                theme.cursor_line_bg
            };
            assert!((1..width - 1).all(|x| buf.get(x, 2).bg == bg));
            assert_eq!(buf.get(1, 2).symbol(), if focused { "▸" } else { " " });
            assert_eq!(buf.get(7, 2).symbol(), "l");
            assert_eq!(buf.get(7, 3).symbol(), "t");
            assert_eq!(buf.get(7, 3).bg, theme.background);
            assert_eq!(buf.get(0, 2).bg, theme.background);
            assert_eq!(buf.get(width - 1, 2).bg, theme.background);
        }
    }
}
