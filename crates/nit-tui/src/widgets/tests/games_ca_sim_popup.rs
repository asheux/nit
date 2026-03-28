use super::*;

#[test]
fn pad_rows_matches_notebook_shape_for_integer_radius() {
    let rows = vec![vec![0, 1, 1, 0], vec![1, 0], vec![1]];
    let padded = pad_rows_for_plot(&rows, 2);
    assert_eq!(padded.len(), 3);
    assert_eq!(padded[0].len(), 4);
    assert_eq!(padded[1], vec![-1, 1, 0, -1]);
    assert_eq!(padded[2], vec![-1, -1, 1, -1]);
}

#[test]
fn legend_includes_output_action() {
    let lines = build_legend_lines(&Theme::default(), 1, Action::Defect);
    let output = lines
        .last()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .unwrap_or_default();
    assert!(output.contains("output:"));
    assert!(output.contains("1 (= D)"));
}
