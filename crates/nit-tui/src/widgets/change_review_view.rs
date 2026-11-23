use std::borrow::Cow;

use nit_core::{state::ChangeReview, AppState, PaneId};
use ratatui::{
    layout::{Position, Rect},
    style::{Modifier, Style},
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
    let suffix = if review.truncated { " · limited" } else { "" };
    let summary = if conflicts > 0 {
        format!("{conflicts} conflicted · S {staged}  W {unstaged}  ? {untracked}{suffix}")
    } else if r.summary.width >= 38 {
        format!("{staged} staged · {unstaged} unstaged · {untracked} untracked{suffix}")
    } else {
        format!("S {staged}  W {unstaged}  ? {untracked}{suffix}")
    };
    frame.render_widget(Paragraph::new(summary).style(muted), r.summary);
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
                format!("{}{} ", file.index, file.worktree),
                style.fg(theme.accent),
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
        frame.render_widget(Paragraph::new(visible(message)).style(muted), r.patch);
    } else if !current {
        frame.render_widget(Paragraph::new("Loading diff…").style(muted), r.patch);
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

fn paint_patch(frame: &mut Frame, review: &ChangeReview, patch: Rect, theme: &Theme) {
    let start = review
        .scroll
        .min(review.preview.len().saturating_sub(patch.height as usize));
    let lines: Vec<Line> = review
        .preview
        .iter()
        .skip(start)
        .take(patch.height as usize)
        .map(|line| {
            let fg = if line.starts_with("@@") {
                theme.accent
            } else if line.starts_with("+++ ") || line.starts_with("--- ") {
                theme.title
            } else if line.starts_with('+') {
                theme.diff_added
            } else if line.starts_with('-') {
                theme.diff_deleted
            } else {
                theme.foreground
            };
            Line::styled(visible(line), Style::default().fg(fg))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), patch);
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
        let theme = Theme::default();
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
            assert!(buffer.content.iter().all(|cell| {
                cell.bg == theme.background || (!empty && cell.bg == theme.selection_bg)
            }));
            let status = buffer.get(1, 1);
            assert_eq!(Some(status.fg), theme.status_idle_style().fg);
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
