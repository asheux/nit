use std::borrow::Cow;

use nit_core::{state::ChangeReview, AppState, PaneId};
use ratatui::{
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::theme::Theme;

mod history;

struct Regions {
    summary: Rect,
    files: Rect,
    patch: Rect,
    footer: Rect,
}

fn regions(area: Rect, count: usize) -> Regions {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let row = |offset, height| Rect::new(inner.x, inner.y + offset, inner.width, height);
    let summary = row(0, inner.height.min(1));
    let footer_height = u16::from(inner.height >= 5);
    let body = inner.height.saturating_sub(summary.height + footer_height);
    let file_rows = (body / 3).max(1).min(body).min(count.min(8) as u16);
    Regions {
        summary,
        files: row(summary.height, file_rows),
        patch: row(summary.height + file_rows, body - file_rows),
        footer: row(inner.height - footer_height, footer_height),
    }
}

fn first_row(review: &ChangeReview, height: u16) -> usize {
    review
        .selected
        .saturating_add(1)
        .saturating_sub(height as usize)
}

// Show control characters in both patch text and paths, including embedded
// terminal escapes. Tabs in patch lines retain a readable indentation.
fn visible(text: &str) -> Cow<'_, str> {
    if !text.chars().any(char::is_control) {
        return Cow::Borrowed(text);
    }
    let mut out = String::new();
    for ch in text.chars() {
        if ch == '\t' {
            out.push_str("    ");
        } else if ch.is_control() {
            out.extend(ch.escape_default());
        } else {
            out.push(ch);
        }
    }
    Cow::Owned(out)
}

fn branch_label(review: &ChangeReview) -> Cow<'_, str> {
    let branch = visible(&review.branch);
    if review.files.is_empty() {
        return branch;
    }
    if review.detached {
        return Cow::Borrowed("HEAD (no branch)");
    }
    if review.unborn {
        return format!("No commits yet on {branch}").into();
    }
    let Some(upstream) = review.upstream.as_deref() else {
        return branch;
    };
    let mut label = format!("{branch}...{}", visible(upstream));
    let mut status = Vec::new();
    if review.upstream_gone {
        status.push("gone".into());
    }
    if review.ahead > 0 {
        status.push(format!("ahead {}", review.ahead));
    }
    if review.behind > 0 {
        status.push(format!("behind {}", review.behind));
    }
    if !status.is_empty() {
        label.push_str(&format!(" [{}]", status.join(", ")));
    }
    label.into()
}

pub fn render(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let review = &state.change_review;
    let focused = state.focus == PaneId::Visualizer;
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.background).fg(theme.foreground))
            .border_type(if focused {
                BorderType::Thick
            } else {
                BorderType::Plain
            })
            .border_style(Style::default().fg(if focused {
                theme.border_focused
            } else {
                theme.border
            }))
            .title(Line::styled(
                format!(" CHANGES  {} ", branch_label(review)),
                Style::default()
                    .fg(theme.title)
                    .add_modifier(Modifier::BOLD),
            )),
        area,
    );
    if review.files.is_empty() {
        history::render(frame, area, review, theme);
        return;
    }
    let r = regions(area, review.files.len());
    for area in [r.summary, r.files] {
        frame.render_widget(
            Block::default().style(Style::default().bg(theme.list_background())),
            area,
        );
    }
    let muted = theme.status_idle_style();
    let mut conflicts = 0;
    let mut staged = 0;
    let mut unstaged = 0;
    let mut untracked = 0;
    for file in &review.files {
        if file.conflicted() {
            conflicts += 1;
        } else if file.index == '?' {
            untracked += 1;
        } else {
            staged += usize::from(file.index != ' ');
            unstaged += usize::from(file.worktree != ' ');
        }
    }
    let labels = if conflicts > 0 {
        vec![
            (format!("{conflicts} conflicted"), conflicts, theme.error),
            (format!("S {staged}"), staged, theme.success),
            (format!("W {unstaged}"), unstaged, theme.diff_modified),
            (format!("? {untracked}"), untracked, theme.hl.r#type),
        ]
    } else if r.summary.width >= 38 {
        vec![
            (format!("{staged} staged"), staged, theme.success),
            (
                format!("{unstaged} unstaged"),
                unstaged,
                theme.diff_modified,
            ),
            (format!("{untracked} untracked"), untracked, theme.hl.r#type),
        ]
    } else {
        vec![
            (format!("S {staged}"), staged, theme.success),
            (format!("W {unstaged}"), unstaged, theme.diff_modified),
            (format!("? {untracked}"), untracked, theme.hl.r#type),
        ]
    };
    let mut summary = Vec::new();
    for (i, (label, count, color)) in labels.into_iter().enumerate() {
        if i > 0 {
            let separator = if conflicts > 0 && i == 1 || conflicts == 0 && r.summary.width >= 38 {
                " · "
            } else {
                "  "
            };
            summary.push(Span::styled(separator, muted));
        }
        summary.push(Span::styled(
            label,
            if count > 0 {
                Style::default().fg(color)
            } else {
                muted
            },
        ));
    }
    if review.truncated {
        summary.push(Span::styled(" · limited", muted));
    }
    frame.render_widget(Paragraph::new(Line::from(summary)), r.summary);
    let first = first_row(review, r.files.height);
    for (row, file) in review
        .files
        .iter()
        .skip(first)
        .take(r.files.height as usize)
        .enumerate()
    {
        let selected = first + row == review.selected;
        let style = if selected {
            Style::default().bg(theme.selection_bg)
        } else {
            Style::default()
        };
        let path = file.path.to_string_lossy();
        let line = Line::from(vec![
            Span::styled(
                file.index.to_string(),
                style.fg(status_color(file.index, true, file.conflicted(), theme)),
            ),
            Span::styled(
                format!("{} ", file.worktree),
                style.fg(status_color(file.worktree, false, file.conflicted(), theme)),
            ),
            Span::styled(visible(&path), style.fg(theme.foreground)),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(r.files.x, r.files.y + row as u16, r.files.width, 1),
        );
    }
    let current = review
        .files
        .get(review.selected)
        .is_some_and(|file| Some(&file.path) == review.preview_path.as_ref());
    if let Some(message) = &review.message {
        frame.render_widget(
            Paragraph::new(visible(message)).style(muted.bg(theme.background)),
            r.patch,
        );
    } else if !current {
        frame.render_widget(
            Paragraph::new("Loading diff…").style(muted.bg(theme.background)),
            r.patch,
        );
    } else {
        paint_patch(frame, review, r.patch, theme);
    }
    let hint = if r.footer.width >= 41 {
        "↑↓ file  PgUp/PgDn diff  Enter open  r sync"
    } else {
        "↑↓ file  Pg↓ diff  ↵ open  r sync"
    };
    frame.render_widget(Paragraph::new(hint).style(muted), r.footer);
}

fn status_color(code: char, staged: bool, conflict: bool, theme: &Theme) -> Color {
    if conflict {
        return theme.error;
    }
    match code {
        'D' => theme.diff_deleted,
        '?' => theme.hl.r#type,
        'A' => theme.diff_added,
        ' ' => theme.muted,
        _ if staged => theme.success,
        _ => theme.diff_modified,
    }
}

fn hunk_boundary(line: &str) -> Option<bool> {
    if line.starts_with("@@") {
        Some(true)
    } else if line.starts_with("diff --")
        || matches!(line, "Staged" | "Unstaged" | "Merge conflict")
    {
        Some(false)
    } else {
        None
    }
}

fn patch_line<'a>(text: &'a str, in_hunk: bool, theme: &Theme) -> Line<'a> {
    let plain = Style::default();
    let heading = plain.add_modifier(Modifier::BOLD);
    let style = match text {
        "Staged" => heading.fg(theme.success),
        "Unstaged" => heading.fg(theme.diff_modified),
        "Merge conflict" => heading.fg(theme.error),
        _ if text.starts_with("@@") => {
            let delimiter = if text.starts_with("@@@") { "@@@" } else { "@@" };
            if let Some((range, context)) = text[delimiter.len()..].split_once(delimiter) {
                return Line::from(vec![
                    Span::styled(
                        format!("{delimiter}{range}{delimiter}"),
                        heading.fg(theme.hl.number),
                    ),
                    Span::styled(visible(context), plain.fg(theme.muted)),
                ]);
            }
            heading.fg(theme.hl.number)
        }
        _ if text.starts_with("diff --") => heading.fg(theme.hl.r#type),
        _ if text.starts_with("+++ ") || text.starts_with("--- ") => {
            // A source line can itself start with "++ " or "-- ". Only the
            // pre-hunk path pair belongs to the patch header.
            if !in_hunk {
                heading.fg(theme.hl.r#type)
            } else if text.starts_with('+') {
                plain.fg(theme.diff_added)
            } else {
                plain.fg(theme.diff_deleted)
            }
        }
        _ if text.starts_with('+') => plain.fg(theme.diff_added),
        _ if text.starts_with('-') => plain.fg(theme.diff_deleted),
        _ if text.starts_with(' ') => plain.fg(theme.foreground),
        _ => plain.fg(theme.muted),
    };
    Line::styled(visible(text), style)
}

fn paint_patch(frame: &mut Frame, review: &ChangeReview, patch: Rect, theme: &Theme) {
    let start = review
        .scroll
        .min(review.preview.len().saturating_sub(patch.height as usize));
    let mut in_hunk = review.preview[..start]
        .iter()
        .rev()
        .find_map(|l| hunk_boundary(l))
        .unwrap_or(false);
    let lines: Vec<_> = review.preview[start..]
        .iter()
        .take(patch.height as usize)
        .map(|line| {
            if let Some(boundary) = hunk_boundary(line) {
                in_hunk = boundary;
            }
            patch_line(line, in_hunk, theme)
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.background)),
        patch,
    );
}

pub(crate) fn select(review: &mut ChangeReview, index: usize) {
    let count = if review.files.is_empty() {
        review.commits.len()
    } else {
        review.files.len()
    };
    let next = index.min(count.saturating_sub(1));
    if next != review.selected {
        review.selected = next;
        review.scroll = 0;
        review.refresh = true;
    }
}

pub(crate) fn move_file(review: &mut ChangeReview, delta: isize) {
    select(review, review.selected.saturating_add_signed(delta));
}

pub(crate) fn scroll_patch(review: &mut ChangeReview, area: Rect, delta: isize, page: bool) {
    let patch = if review.files.is_empty() {
        history::layout(area, review).patch
    } else {
        regions(area, review.files.len()).patch
    };
    let rows = patch.height.max(1) as usize;
    let amount = if page {
        delta.saturating_mul(rows as isize)
    } else {
        delta
    };
    review.scroll = review
        .scroll
        .saturating_add_signed(amount)
        .min(review.preview.len().saturating_sub(rows));
}

pub(crate) fn mouse_select(review: &mut ChangeReview, area: Rect, point: Position) {
    if review.files.is_empty() {
        if history::click(review, area, point) {
            return;
        }
        let rows = history::layout(area, review).commits;
        if rows.contains(point) {
            select(
                review,
                first_row(review, rows.height) + usize::from(point.y - rows.y),
            );
        }
        return;
    }
    let r = regions(area, review.files.len());
    if r.files.contains(point) {
        select(
            review,
            first_row(review, r.files.height) + usize::from(point.y - r.files.y),
        );
    }
}

pub(crate) fn mouse_scroll(review: &mut ChangeReview, area: Rect, point: Position, delta: i32) {
    if review.files.is_empty() {
        let r = history::layout(area, review);
        if r.commits.contains(point) {
            move_file(review, delta as isize);
        } else if r.patch.contains(point) {
            scroll_patch(review, area, delta as isize, false);
        }
        return;
    }
    if regions(area, review.files.len()).files.contains(point) {
        move_file(review, delta as isize);
    } else {
        scroll_patch(review, area, delta as isize, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nit_core::{state::ChangedFile, Buffer};
    use ratatui::{backend::TestBackend, Terminal};

    fn state() -> AppState {
        let mut state = AppState::new(
            ".".into(),
            Buffer::empty("e", None),
            Buffer::empty("n", None),
        );
        state.change_review.files = vec![
            ChangedFile {
                path: "one.rs".into(),
                index: 'M',
                worktree: 'M',
            },
            ChangedFile {
                path: "new\nfile".into(),
                index: '?',
                worktree: '?',
            },
        ];
        state.change_review.preview_path = Some("one.rs".into());
        state.change_review.preview = vec!["@@ -1 +1 @@".into(), "-old".into(), "+new".into()];
        state
    }

    fn screen(state: &AppState, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| render(f, f.size(), state, &Theme::default()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(width as usize)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn review_paints_the_theme_on_empty_and_populated_worktrees() {
        let theme = Theme {
            background: Color::Rgb(9, 19, 29),
            editor_background: Color::Rgb(70, 81, 92),
            ..Theme::default()
        };
        let mut state = state();
        let mut terminal = Terminal::new(TestBackend::new(64, 15)).unwrap();
        for empty in [false, true] {
            if empty {
                state.change_review.files.clear();
            }
            terminal
                .draw(|frame| render(frame, frame.size(), &state, &theme))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let r = regions(buffer.area, state.change_review.files.len());
            let list_area = if empty {
                history::layout(buffer.area, &state.change_review).list_area()
            } else {
                r.summary.union(r.files)
            };
            for y in 0..buffer.area.height {
                for x in 0..buffer.area.width {
                    let point = Position::new(x, y);
                    let expected = if !empty && r.files.contains(point) && y == r.files.y {
                        theme.selection_bg
                    } else if list_area.contains(point) {
                        theme.list_background()
                    } else {
                        theme.background
                    };
                    assert_eq!(buffer.get(x, y).bg, expected, "{point:?}");
                }
            }
            let status = buffer.get(1, 1);
            assert_eq!(status.fg, theme.success);
            assert!(!status.modifier.contains(Modifier::DIM));
        }
    }

    #[test]
    fn review_shows_status_counts_and_only_the_selected_patch() {
        let mut state = state();
        let output = screen(&state, 62, 15);
        for s in [
            "1 staged · 1 unstaged · 1 untracked",
            "MM one.rs",
            "?? new\\nfile",
            "-old",
            "+new",
        ] {
            assert!(output.contains(s), "{s}: {output}");
        }
        let area = Rect::new(0, 0, 62, 15);
        let rows = regions(area, 2).files;
        mouse_select(
            &mut state.change_review,
            area,
            Position::new(rows.x, rows.y + 1),
        );
        assert_eq!(state.change_review.selected, 1);
        assert!(state.change_review.refresh);
        let output = screen(&state, 22, 10);
        assert!(output.contains("Loading diff"));
        assert!(!output.contains("-old"));
        screen(&state, 3, 2);
    }

    #[test]
    fn review_scroll_bounds_match_visible_rows() {
        let mut state = state();
        let area = Rect::new(0, 0, 35, 10);
        state.change_review.preview = vec!["context".into(); 20];
        let visible = regions(area, 2).patch.height as usize;
        scroll_patch(&mut state.change_review, area, 1, true);
        assert_eq!(state.change_review.scroll, visible);
        scroll_patch(&mut state.change_review, area, 100, false);
        assert_eq!(state.change_review.scroll, 20 - visible);
        move_file(&mut state.change_review, 1);
        assert_eq!(state.change_review.scroll, 0);
    }
}
