use super::*;
use nit_core::{UiSelection, UiSelectionPane};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) fn ready(review: &ChangeReview) -> bool {
    if let Some(file) = review.files.get(review.selected) {
        review.preview_path.as_ref() == Some(&file.path)
    } else {
        review
            .commits
            .get(review.selected)
            .is_some_and(|c| review.preview_commit.as_ref() == Some(&c.id))
    }
}

pub(crate) fn patch_area(review: &ChangeReview, area: Rect) -> Rect {
    if review.files.is_empty() {
        history::layout(area, review).patch
    } else {
        regions(area, review.files.len()).patch
    }
}

pub(crate) fn follow_hit(review: &ChangeReview, area: Rect, point: Position) -> bool {
    let (footer, x) = if review.files.is_empty() {
        let r = history::layout(area, review);
        (
            r.footer,
            history::action(review, r.footer.width).width() as u16 + 2,
        )
    } else {
        (regions(area, review.files.len()).footer, 0)
    };
    let width = "f Follow up".width() as u16;
    footer.contains(point)
        && x + width <= footer.width
        && (x..x + width).contains(&(point.x - footer.x))
}

fn char_width(ch: char) -> usize {
    if ch == '\t' {
        4
    } else if ch.is_control() {
        ch.escape_default().count()
    } else {
        ch.width().unwrap_or(0)
    }
}

pub(crate) fn position(
    review: &ChangeReview,
    area: Rect,
    point: Position,
    clamp: bool,
) -> Option<(usize, usize)> {
    let patch = patch_area(review, area);
    if patch.is_empty()
        || !ready(review)
        || review.preview.is_empty()
        || (!clamp && !patch.contains(point))
    {
        return None;
    }
    let top = review
        .scroll
        .min(review.preview.len().saturating_sub(patch.height as usize));
    let index = top + usize::from(point.y.clamp(patch.y, patch.bottom() - 1) - patch.y);
    if !clamp && index >= review.preview.len() {
        return None;
    }
    let index = index.min(review.preview.len() - 1);
    let x = usize::from(point.x.clamp(patch.x, patch.right() - 1) - patch.x)
        + usize::from(review.horizontal_scroll);
    let mut cell = 0;
    let col = review.preview[index]
        .chars()
        .take_while(|ch| {
            cell += char_width(*ch);
            cell <= x
        })
        .count();
    Some((index, col))
}

pub(crate) fn horizontal(review: &mut ChangeReview, area: Rect, delta: i32) {
    let width = usize::from(patch_area(review, area).width);
    let max = review
        .preview
        .iter()
        .map(|s| s.chars().map(char_width).sum::<usize>())
        .max()
        .unwrap_or(0)
        .saturating_sub(width)
        .min(u16::MAX as usize);
    review.horizontal_scroll =
        (i32::from(review.horizontal_scroll) + delta).clamp(0, max as i32) as u16;
}

pub(super) fn highlight(
    state: &AppState,
    lines: Vec<Line<'static>>,
    top: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let Some(mut sel) = state
        .ui_selection
        .filter(|s| s.pane == UiSelectionPane::ChangesPatch)
    else {
        return lines;
    };
    let display_col = |row: usize, col: usize| {
        state.change_review.preview.get(row).map_or(0, |line| {
            line.chars()
                .take(col)
                .map(|ch| {
                    if ch == '\t' {
                        4
                    } else if ch.is_control() {
                        ch.escape_default().count()
                    } else {
                        1
                    }
                })
                .sum()
        })
    };
    sel.start_col = display_col(sel.start_line, sel.start_col);
    sel.end_col = display_col(sel.end_line, sel.end_col);
    let styles: Vec<_> = lines.iter().map(|line| line.style).collect();
    let mut marked = crate::widgets::text_selection::apply_ui_selection(
        lines,
        Some(&sel),
        UiSelectionPane::ChangesPatch,
        theme.selection_bg,
        top,
    );
    for (line, style) in marked.iter_mut().zip(styles) {
        line.style = style;
    }
    marked
}

pub(crate) fn selected(state: &AppState) -> Option<UiSelection> {
    state
        .ui_selection
        .filter(|s| s.pane == UiSelectionPane::ChangesPatch && ready(&state.change_review))
}

/// Git prefixes both sides of a patch path and may C-quote their bytes.
fn path_header(line: &str) -> Option<String> {
    let raw = line.trim_end_matches('\t');
    let decoded = if let Some(raw) = raw.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        let mut bytes = Vec::new();
        let mut it = raw.bytes();
        while let Some(b) = it.next() {
            if b != b'\\' {
                bytes.push(b);
                continue;
            }
            let n = it.next()?;
            bytes.push(match n {
                b'0'..=b'3' => {
                    let lo = it.next()?.checked_sub(b'0')?;
                    let hi = it.next()?.checked_sub(b'0')?;
                    if lo > 7 || hi > 7 {
                        return None;
                    }
                    ((n - b'0') << 6) | (lo << 3) | hi
                }
                b't' => b'\t',
                b'n' => b'\n',
                b'r' => b'\r',
                b'v' => 11,
                b'f' => 12,
                b'a' => 7,
                b'b' => 8,
                b'"' | b'\\' => n,
                _ => return None,
            });
        }
        String::from_utf8(bytes).ok()?
    } else {
        raw.to_owned()
    };
    decoded
        .strip_prefix("b/")
        .or_else(|| decoded.strip_prefix("a/"))
        .map(str::to_owned)
}

pub(crate) fn context(state: &AppState) -> Option<(String, Option<String>, Option<String>)> {
    let r = &state.change_review;
    let mut path = r
        .files
        .get(r.selected)
        .map(|f| f.path.to_string_lossy().into_owned());
    let revision = if path.is_none() {
        Some(r.commits.get(r.selected)?.id.clone())
    } else {
        None
    };
    let mut hunk = None;
    if ready(r) {
        let index = selected(state).map_or(r.scroll, |s| s.start_line.min(s.end_line));
        let mut header = true;
        for row in r.preview.iter().take(index.saturating_add(1)) {
            if row.starts_with("diff --git ")
                || matches!(row.as_str(), "Staged" | "Unstaged" | "Merge conflict")
            {
                hunk = None;
                header = true;
                if revision.is_some() {
                    path = None;
                }
            } else if row.starts_with("@@") {
                hunk = Some(row.clone());
                header = false;
            } else if header && revision.is_some() {
                if let Some(raw) = row
                    .strip_prefix("+++ ")
                    .or_else(|| row.strip_prefix("--- "))
                {
                    if let Some(p) = path_header(raw) {
                        path = Some(p);
                    }
                }
            }
        }
    }
    Some((path.unwrap_or_default(), revision, hunk))
}

/// Unified diff counters are one-based and advance independently for removed and added lines.
pub(crate) fn source_line(state: &AppState) -> Option<(bool, usize)> {
    let r = &state.change_review;
    if !ready(r) {
        return None;
    }
    let i = selected(state).map_or(r.scroll, |s| s.start_line.min(s.end_line));
    let mut lines = r.preview.get(..=i)?.iter().rev();
    let header = lines.position(|s| {
        s.starts_with("@@")
            || s.starts_with("diff --git ")
            || matches!(s.as_str(), "Staged" | "Unstaged" | "Merge conflict")
    })?;
    let start = i - header;
    let mut ranges = r.preview[start].strip_prefix("@@ ")?.split_whitespace();
    let mut old = ranges
        .next()?
        .strip_prefix('-')?
        .split(',')
        .next()?
        .parse::<usize>()
        .ok()?;
    let mut new = ranges
        .next()?
        .strip_prefix('+')?
        .split(',')
        .next()?
        .parse::<usize>()
        .ok()?;
    for (offset, line) in r.preview[start + 1..=i].iter().enumerate() {
        let sign = *line.as_bytes().first()?;
        if !matches!(sign, b'+' | b'-' | b' ') {
            continue;
        }
        if start + 1 + offset == i {
            return Some(if sign == b'-' {
                (true, old)
            } else {
                (false, new)
            });
        }
        old += usize::from(sign != b'+');
        new += usize::from(sign != b'-');
    }
    None
}
