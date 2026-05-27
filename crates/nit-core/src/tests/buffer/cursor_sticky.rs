use super::*;

fn buffer_with_mixed_lines() -> Buffer {
    let content = "first long line is plenty wide\nshort\nx\nanother very long line here\n";
    Buffer::from_str("sticky", content, None)
}

#[test]
fn move_down_through_short_line_restores_column() {
    let mut buf = buffer_with_mixed_lines();
    buf.cursor.line = 0;
    buf.cursor.col = 12;

    buf.move_down();
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, buf.line_char_len(1));
    assert_eq!(buf.cursor.desired_col, Some(12));

    buf.move_down();
    assert_eq!(buf.cursor.line, 2);
    assert_eq!(buf.cursor.col, buf.line_char_len(2));
    assert_eq!(buf.cursor.desired_col, Some(12));

    buf.move_down();
    assert_eq!(buf.cursor.line, 3);
    assert_eq!(buf.cursor.col, 12);
    assert_eq!(buf.cursor.desired_col, Some(12));
}

#[test]
fn move_up_through_short_line_restores_column() {
    let mut buf = buffer_with_mixed_lines();
    buf.cursor.line = 3;
    buf.cursor.col = 20;

    buf.move_up();
    assert_eq!(buf.cursor.line, 2);
    assert_eq!(buf.cursor.col, buf.line_char_len(2));
    assert_eq!(buf.cursor.desired_col, Some(20));

    buf.move_up();
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, buf.line_char_len(1));
    assert_eq!(buf.cursor.desired_col, Some(20));

    buf.move_up();
    assert_eq!(buf.cursor.line, 0);
    assert_eq!(buf.cursor.col, 20);
}

#[test]
fn horizontal_motion_clears_desired_col_then_anchors_to_new_col() {
    let mut buf = buffer_with_mixed_lines();
    buf.cursor.line = 0;
    buf.cursor.col = 12;

    buf.move_down();
    assert_eq!(buf.cursor.desired_col, Some(12));

    buf.move_right();
    assert_eq!(buf.cursor.desired_col, None);

    buf.cursor.line = 0;
    buf.cursor.col = 5;
    buf.move_down();
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, 5);
    assert_eq!(buf.cursor.desired_col, Some(5));
}

#[test]
fn ctrl_d_ctrl_u_preserve_desired_col() {
    let line = "x".repeat(40);
    let short = "short".to_string();
    let content = format!("{line}\n{short}\n{short}\n{short}\n{short}\n{line}\n");
    let mut buf = Buffer::from_str("sticky-scroll", &content, None);
    buf.set_viewport_size(4, 80);
    buf.cursor.line = 0;
    buf.cursor.col = 30;

    buf.scroll_half_page_down();
    assert_eq!(buf.cursor.line, 2);
    assert_eq!(buf.cursor.col, buf.line_char_len(2));
    assert_eq!(buf.cursor.desired_col, Some(30));

    buf.scroll_half_page_down();
    assert_eq!(buf.cursor.line, 4);
    assert_eq!(buf.cursor.col, buf.line_char_len(4));
    assert_eq!(buf.cursor.desired_col, Some(30));

    buf.scroll_half_page_down();
    assert_eq!(buf.cursor.line, 5);
    assert_eq!(buf.cursor.col, 30);

    buf.scroll_half_page_up();
    buf.scroll_half_page_up();
    buf.scroll_half_page_up();
    assert_eq!(buf.cursor.line, 0);
    assert_eq!(buf.cursor.col, 30);
}

#[test]
fn page_down_then_page_up_round_trips_column() {
    let line = "abcdefghijklmnop".to_string();
    let short = "s".to_string();
    let content = format!("{line}\n{short}\n{short}\n{short}\n{line}\n");
    let mut buf = Buffer::from_str("page", &content, None);
    buf.cursor.line = 0;
    buf.cursor.col = 10;

    buf.page_down(2);
    assert_eq!(buf.cursor.line, 2);
    assert_eq!(buf.cursor.col, buf.line_char_len(2));
    assert_eq!(buf.cursor.desired_col, Some(10));

    buf.page_down(2);
    assert_eq!(buf.cursor.line, 4);
    assert_eq!(buf.cursor.col, 10);

    buf.page_up(3);
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, buf.line_char_len(1));
    assert_eq!(buf.cursor.desired_col, Some(10));

    buf.page_up(1);
    assert_eq!(buf.cursor.line, 0);
    assert_eq!(buf.cursor.col, 10);
}

#[test]
fn go_to_line_preserves_desired_col_anchor() {
    let line = "x".repeat(30);
    let content = format!("{line}\nshort\n{line}\nshort\n{line}\n");
    let mut buf = Buffer::from_str("go", &content, None);
    buf.cursor.line = 0;
    buf.cursor.col = 18;

    buf.go_to_line(2);
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, buf.line_char_len(1));
    assert_eq!(buf.cursor.desired_col, Some(18));

    buf.go_to_line(3);
    assert_eq!(buf.cursor.line, 2);
    assert_eq!(buf.cursor.col, 18);
}

#[test]
fn vertical_motion_at_buffer_boundary_does_not_corrupt_desired_col() {
    let mut buf = buffer_with_mixed_lines();
    buf.cursor.line = 0;
    buf.cursor.col = 12;

    buf.move_up();
    assert_eq!(buf.cursor.line, 0);
    assert_eq!(buf.cursor.col, 12);
    assert_eq!(buf.cursor.desired_col, None);

    buf.move_down();
    assert_eq!(buf.cursor.line, 1);
    assert_eq!(buf.cursor.col, buf.line_char_len(1));
    assert_eq!(buf.cursor.desired_col, Some(12));
}

#[test]
fn move_end_then_down_does_not_stick_to_end() {
    let content = "alpha-beta-gamma\nshort line here\n";
    let mut buf = Buffer::from_str("end-down", content, None);
    buf.cursor.line = 0;
    buf.cursor.col = 0;
    buf.move_end();
    let first_end = buf.cursor.col;
    assert_eq!(buf.cursor.desired_col, None);

    buf.move_down();
    assert_eq!(buf.cursor.line, 1);
    // Line 1 is shorter than line 0's end, so the cursor clamps visually
    // while desired_col preserves the original end-of-line anchor — a
    // subsequent move_down to a longer line will restore col == first_end.
    assert_eq!(buf.cursor.col, buf.line_char_len(1));
    assert_eq!(buf.cursor.desired_col, Some(first_end));
}
