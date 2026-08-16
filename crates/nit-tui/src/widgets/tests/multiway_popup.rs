use super::*;
use nit_core::GenomeTier;

fn node(
    short: &str,
    depth: u16,
    status: MultiwayNodeStatus,
    score: Option<f32>,
) -> MultiwayNodeView {
    MultiwayNodeView {
        id: format!("commit-{short}"),
        short_id: short.to_string(),
        depth,
        status,
        score,
        tier: score.map(|_| GenomeTier::Spaceship),
        summary: format!("turn summary for {short}"),
        changed: 2,
        parents: vec![],
    }
}

fn view_of(
    nodes: Vec<MultiwayNodeView>,
    kept: Vec<String>,
    in_flight: Option<String>,
) -> MultiwayView {
    MultiwayView {
        header: MultiwayHeader {
            mood: "balanced".to_string(),
            k: 3,
            turns: 5,
            max_turns: 40,
            nodes: nodes.len(),
            max_nodes: 200,
            frontier: 2,
            best_score: 0.8,
            best_tier: GenomeTier::Methuselah,
            stop_reason: None,
        },
        nodes,
        kept_path: kept,
        in_flight,
    }
}

fn row_text(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn value_colour_ramps_red_amber_green_and_clamps() {
    let theme = Theme::default();
    assert_eq!(value_color(0.0, &theme), theme.error);
    assert_eq!(value_color(0.5, &theme), theme.warning);
    assert_eq!(value_color(1.0, &theme), theme.success);
    assert_eq!(value_color(-3.0, &theme), theme.error);
    assert_eq!(value_color(9.0, &theme), theme.success);
}

#[test]
fn status_glyphs_match_spec() {
    assert_eq!(status_glyph(MultiwayNodeStatus::Open), "●");
    assert_eq!(status_glyph(MultiwayNodeStatus::Held), "⊙");
    assert_eq!(status_glyph(MultiwayNodeStatus::Expanded), "○");
    assert_eq!(status_glyph(MultiwayNodeStatus::GateFailed), "✗");
    assert_eq!(status_glyph(MultiwayNodeStatus::Solution), "★");
    assert_eq!(status_glyph(MultiwayNodeStatus::Dominated), "◌");
}

#[test]
fn node_row_carries_status_glyph_and_value_colour() {
    let theme = Theme::default();
    let v = view_of(
        vec![node("a1", 0, MultiwayNodeStatus::Open, Some(1.0))],
        vec![],
        None,
    );
    let rows = tree_rows(&v, &theme, 120);
    assert_eq!(rows.len(), 1);
    let text = row_text(&rows[0]);
    assert!(text.contains('●'), "frontier glyph present: {text}");
    assert!(text.contains("a1"));
    // A perfect score paints the id/score columns green (theme.success).
    let id_span = rows[0]
        .spans
        .iter()
        .find(|s| s.content.starts_with("a1"))
        .expect("short-id span");
    assert_eq!(id_span.style.fg, Some(theme.success));
}

#[test]
fn kept_path_row_is_gold_and_bold() {
    let theme = Theme::default();
    let v = view_of(
        vec![node("k9", 0, MultiwayNodeStatus::Solution, Some(0.9))],
        vec!["commit-k9".to_string()],
        None,
    );
    let rows = tree_rows(&v, &theme, 120);
    let glyph = rows[0]
        .spans
        .iter()
        .find(|s| s.content.contains('★'))
        .expect("solution glyph");
    assert_eq!(glyph.style.fg, Some(theme.accent));
    assert!(glyph.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn in_flight_node_shows_spinner_and_next_tag() {
    let theme = Theme::default();
    let v = view_of(
        vec![node("f2", 0, MultiwayNodeStatus::Open, Some(0.5))],
        vec![],
        Some("f2".to_string()),
    );
    let rows = tree_rows(&v, &theme, 120);
    let text = row_text(&rows[0]);
    assert!(text.contains(SPINNER), "spinner present: {text}");
    assert!(text.contains("next"), "next tag present: {text}");
    let glyph = rows[0]
        .spans
        .iter()
        .find(|s| s.content.contains(SPINNER))
        .expect("spinner span");
    assert_eq!(glyph.style.fg, Some(theme.accent));
}

#[test]
fn merge_node_shows_multi_parent_ref() {
    let theme = Theme::default();
    let mut merge = node("m4", 0, MultiwayNodeStatus::Expanded, Some(0.6));
    merge.parents = vec!["commit-a".to_string(), "commit-b".to_string()];
    let rows = tree_rows(&view_of(vec![merge], vec![], None), &theme, 120);
    assert!(row_text(&rows[0]).contains("⇄2"), "{}", row_text(&rows[0]));
}

#[test]
fn deep_subtree_collapses_into_a_single_marker() {
    let theme = Theme::default();
    let nodes: Vec<MultiwayNodeView> = (0u16..=9)
        .map(|d| node(&format!("d{d}"), d, MultiwayNodeStatus::Expanded, Some(0.3)))
        .collect();
    let rows = tree_rows(&view_of(nodes, vec![], None), &theme, 120);
    // Depths 0..=6 render as seven rows; depths 7,8,9 fold into one marker.
    assert_eq!(rows.len(), 8);
    let marker = row_text(rows.last().expect("marker row"));
    assert!(marker.contains("collapsed"), "marker: {marker}");
    assert!(marker.contains('3'), "hidden count in marker: {marker}");
}

#[test]
fn empty_view_renders_idle_hint() {
    let theme = Theme::default();
    let rows = tree_rows(&view_of(vec![], vec![], None), &theme, 120);
    assert_eq!(rows.len(), 1);
    assert!(row_text(&rows[0]).contains("no multiway search"));
}

#[test]
fn header_shows_mood_budget_and_stop_reason() {
    let theme = Theme::default();
    let mut v = view_of(vec![], vec![], None);
    v.header.stop_reason = Some("budget exhausted".to_string());
    let lines = header_lines(&v.header, &theme);
    assert_eq!(lines.len(), 2);
    let top = row_text(&lines[0]);
    assert!(top.contains("balanced·k3"), "{top}");
    assert!(top.contains("5/40"), "{top}");
    assert!(top.contains("200"), "{top}");
    let bottom = row_text(&lines[1]);
    assert!(bottom.contains("budget exhausted"), "{bottom}");
    assert!(bottom.contains("IV"), "best tier numeral: {bottom}");
}

#[test]
fn render_via_agents_state_draws_title_header_and_glyphs() {
    use nit_core::buffer::Buffer;
    use nit_core::AppState;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let root = std::env::temp_dir().join(format!(
        "nit-multiway-popup-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("temp dir");
    let mut state = AppState::new(root, Buffer::empty("x", None), Buffer::empty("n", None));
    let theme = Theme::default();
    let area = Rect::new(0, 0, 80, 20);
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).expect("terminal");

    // The frozen seam reads the live view-model out of `&state.agents`.
    state.agents.multiway_view = Some(view_of(
        vec![
            node("r0", 0, MultiwayNodeStatus::Solution, Some(0.95)),
            node("c1", 1, MultiwayNodeStatus::Open, Some(0.4)),
        ],
        vec!["commit-r0".to_string()],
        Some("c1".to_string()),
    ));
    terminal
        .draw(|frame| render(frame, area, &state.agents, &theme))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let mut content = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            content.push_str(buffer.get(x, y).symbol());
        }
        content.push('\n');
    }
    assert!(content.contains("MULTIWAY SEARCH"), "title:\n{content}");
    assert!(content.contains("balanced·k3"), "header:\n{content}");
    assert!(content.contains('★'), "solution glyph:\n{content}");
    assert!(content.contains(SPINNER), "spinner glyph:\n{content}");

    // No snapshot streamed yet: the seam still paints chrome + an idle hint (not
    // nothing), so the modal popup is visible rather than an invisible key-trap.
    state.agents.multiway_view = None;
    terminal
        .draw(|frame| render(frame, area, &state.agents, &theme))
        .expect("draw idle");
    let buffer = terminal.backend().buffer();
    let mut idle = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            idle.push_str(buffer.get(x, y).symbol());
        }
    }
    assert!(idle.contains("MULTIWAY SEARCH"), "idle chrome:\n{idle}");
    assert!(
        idle.contains("no multiway search active"),
        "idle hint:\n{idle}"
    );
}

#[test]
fn untrusted_summary_control_chars_are_neutralised() {
    let theme = Theme::default();
    let mut n = node("u1", 0, MultiwayNodeStatus::Open, Some(0.5));
    n.summary = "a\x1b[2Jb\tc\r".to_string();
    let rows = tree_rows(&view_of(vec![n], vec![], None), &theme, 120);
    let text = row_text(&rows[0]);
    assert!(
        !text.chars().any(char::is_control),
        "control chars must not survive into a rendered row: {text:?}"
    );
    // The ESC is neutralised, not the printable text it preceded.
    assert!(text.contains('b') && text.contains('c'), "{text:?}");
}

/// The acceptance test the draw-gap slipped past: the widget render was proven
/// in isolation, but nothing exercised the `show_multiway_popup` gate that
/// `app/draw.rs` uses to decide whether to paint at all. This mirrors that exact
/// gate (`preferred_size` → `render`, keyed on the toggle) so a regression that
/// stops painting the popup — or paints it while closed — fails here.
#[test]
fn draw_gate_paints_popup_only_when_show_flag_set() {
    use nit_core::buffer::Buffer;
    use nit_core::AppState;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let root = std::env::temp_dir().join(format!(
        "nit-multiway-gate-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("temp dir");
    let mut state = AppState::new(root, Buffer::empty("x", None), Buffer::empty("n", None));
    let theme = Theme::default();
    state.agents.multiway_view = Some(view_of(
        vec![node("g0", 0, MultiwayNodeStatus::Open, Some(0.7))],
        vec![],
        None,
    ));

    let paint = |state: &AppState| -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).expect("terminal");
        terminal
            .draw(|f| {
                let screen = f.size();
                if state.agents.show_multiway_popup {
                    let area = preferred_size(screen);
                    render(f, area, &state.agents, &theme);
                }
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut content = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                content.push_str(buffer.get(x, y).symbol());
            }
        }
        content
    };

    // Toggle off → the gate skips render even with a streamed view: nothing painted.
    state.agents.show_multiway_popup = false;
    assert!(
        !paint(&state).contains("MULTIWAY SEARCH"),
        "popup must not paint while the toggle is off"
    );

    // Toggle on + a streamed view → the popup paints its title.
    state.agents.show_multiway_popup = true;
    assert!(
        paint(&state).contains("MULTIWAY SEARCH"),
        "popup must paint once the toggle is set and a view has streamed"
    );
}
