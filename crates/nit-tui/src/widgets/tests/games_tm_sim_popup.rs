use super::*;

fn line_to_string(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

#[test]
fn rule_table_renders_with_borders() {
    let transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Right,
            next: 1,
        },
        TmTransition {
            write: 1,
            move_dir: TmMove::Left,
            next: 1,
        },
        TmTransition {
            write: 1,
            move_dir: TmMove::Stay,
            next: 0,
        },
        TmTransition {
            write: 0,
            move_dir: TmMove::Right,
            next: 2,
        },
    ];
    let lines = build_rule_table_lines(2, 2, &transitions, Style::default(), Style::default(), 80);
    assert_eq!(lines.len(), 8);
    let top = line_to_string(&lines[0]);
    let header = line_to_string(&lines[1]);
    let mid = line_to_string(&lines[2]);
    let bottom = line_to_string(lines.last().unwrap());
    assert!(top.starts_with('+') && top.ends_with('+'));
    assert!(mid.starts_with('+') && mid.ends_with('+'));
    assert!(bottom.starts_with('+') && bottom.ends_with('+'));
    assert_eq!(top.len(), bottom.len());
    assert_eq!(top.len(), header.len());
    assert!(header.contains("| {s, r} "));
    assert!(header.contains("| {n, w, m} "));
    let row = line_to_string(&lines[3]);
    assert!(row.contains("{1, 0}"));
}

#[test]
fn step_table_renders_with_borders_and_tape() {
    let steps = vec![SimStep {
        step: 1,
        state: 2,
        head_before: 1,
        read: 1,
        next: 2,
        write: 0,
        move_dir: TmMove::Right,
        head_after: 2,
        tape: vec![0, 1, 1, 0],
    }];
    let lines = build_step_table_lines(&steps, 120, Style::default(), Style::default());
    assert_eq!(lines.len(), 5);
    let top = line_to_string(&lines[0]);
    let header = line_to_string(&lines[1]);
    let row = line_to_string(&lines[3]);
    let bottom = line_to_string(lines.last().unwrap());
    assert!(top.starts_with('+') && top.ends_with('+'));
    assert!(bottom.starts_with('+') && bottom.ends_with('+'));
    assert_eq!(top.len(), bottom.len());
    assert_eq!(top.len(), header.len());
    assert!(header.contains("| tape "));
    assert!(row.contains('●'));
}

#[test]
fn evolution_head_clamps_at_left_edge() {
    let transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Left,
            next: 1,
        },
        TmTransition {
            write: 0,
            move_dir: TmMove::Left,
            next: 1,
        },
    ];
    let output_map = vec![Action::Cooperate, Action::Defect];
    let sim = simulate_tm(0, 2, 1, 0, 0, 8, &transitions, &output_map);
    assert!(sim.frames.iter().all(|frame| frame.origin == 0));
    let mut clamped = false;
    for window in sim.frames.windows(2) {
        if window[0].head == 0 && window[1].head == 0 {
            clamped = true;
            break;
        }
    }
    assert!(clamped, "expected head to clamp at left boundary");
}

#[test]
fn non_halting_evolution_truncates_at_fixed_point() {
    let transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Stay,
            next: 1,
        },
        TmTransition {
            write: 1,
            move_dir: TmMove::Stay,
            next: 1,
        },
    ];
    let output_map = vec![Action::Cooperate, Action::Defect];
    let sim = simulate_tm(0, 2, 1, 0, 0, 64, &transitions, &output_map);
    assert!(!sim.halted);
    assert!(sim.frames.len() < 65);
    assert_eq!(sim.frames.len(), 3);
    assert!(sim
        .log_lines
        .iter()
        .any(|line| line.contains("fixed point at step 2")));
}

#[test]
fn rules_table_border_aligns_in_right_column() {
    let transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Right,
            next: 1,
        },
        TmTransition {
            write: 1,
            move_dir: TmMove::Left,
            next: 1,
        },
        TmTransition {
            write: 1,
            move_dir: TmMove::Stay,
            next: 0,
        },
        TmTransition {
            write: 0,
            move_dir: TmMove::Right,
            next: 2,
        },
    ];
    let right_width = 32usize;
    let right_lines = build_rule_table_lines(
        2,
        2,
        &transitions,
        Style::default(),
        Style::default(),
        right_width,
    );
    assert!(!right_lines.is_empty());
    let left_width = 20usize;
    let gap = 2usize;
    let left_lines = vec![Line::from(""); right_lines.len()];
    let merged = merge_columns(left_lines, right_lines, left_width, right_width, gap);
    let border = line_to_string(&merged[0]);
    let idx = border.find('+').unwrap_or(usize::MAX);
    assert_eq!(idx, left_width + gap);
}

#[test]
fn steps_table_border_aligns_in_left_column() {
    let steps = vec![SimStep {
        step: 1,
        state: 2,
        head_before: 1,
        read: 1,
        next: 2,
        write: 0,
        move_dir: TmMove::Right,
        head_after: 2,
        tape: vec![0, 1, 1, 0],
    }];
    let right_width = 48usize;
    let right_lines =
        build_step_table_lines(&steps, right_width, Style::default(), Style::default());
    assert!(!right_lines.is_empty());
    let left_width = 20usize;
    let gap = 2usize;
    let merged = merge_columns(right_lines, Vec::new(), left_width, right_width, gap);
    let border = line_to_string(&merged[0]);
    let idx = border.find('+').unwrap_or(usize::MAX);
    assert_eq!(idx, 0);
}

#[test]
fn legend_shows_halt_and_output_value() {
    let lines = build_legend_lines(2, &Theme::default(), true, Some(7), Some(1));
    let summary = line_to_string(&lines[1]);
    assert!(summary.contains("halt:"));
    assert!(summary.contains("true"));
    assert!(summary.contains("output:"));
    assert!(summary.contains("7"));
    assert!(summary.contains("mod 2 = 1"));
    assert!(summary.contains("= D"));
}

#[test]
fn legend_shows_timeout_for_non_halting_run() {
    let lines = build_legend_lines(2, &Theme::default(), false, None, None);
    let summary = line_to_string(&lines[1]);
    assert!(summary.contains("false"));
    assert!(summary.contains("timeout -> D"));
}
