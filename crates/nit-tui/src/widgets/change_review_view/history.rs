use super::*;
use unicode_width::UnicodeWidthStr;

pub(super) struct Layout {
    summary: Rect,
    tracking: Rect,
    heading: Rect,
    pub(super) commits: Rect,
    detail: Rect,
    pub(super) patch: Rect,
    footer: Rect,
}

impl Layout {
    pub(super) fn list_area(&self) -> Rect {
        Rect::new(
            self.summary.x,
            self.summary.y,
            self.summary.width,
            self.summary.height + self.tracking.height + self.heading.height + self.commits.height,
        )
    }
}

pub(super) fn layout(area: Rect, review: &ChangeReview) -> Layout {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let mut body = inner;
    let footer = if body.height >= 5 {
        body.height -= 1;
        Rect::new(body.x, body.bottom(), body.width, 1)
    } else {
        Rect::default()
    };
    let mut take = |n: u16| {
        let h = n.min(body.height);
        let result = Rect::new(body.x, body.y, body.width, h);
        body.y += h;
        body.height -= h;
        result
    };
    let summary = take(1);
    let tracking = take(u16::from(inner.height >= 8 && !review.history_patch_open));
    let expanded = review.history_patch_open && !review.commits.is_empty();
    let heading = take(u16::from(!expanded && inner.height >= 9));
    let remaining = inner
        .height
        .saturating_sub(summary.height + tracking.height + heading.height + footer.height);
    let count = if expanded {
        0
    } else if remaining < 4 {
        remaining
    } else {
        (remaining / 2).clamp(2, 6)
    };
    let commits = take(count.min(review.commits.len() as u16));
    let detail = take(u16::from(!review.commits.is_empty()));
    let patch = body;
    Layout {
        summary,
        tracking,
        heading,
        commits,
        detail,
        patch,
        footer,
    }
}

fn tracking(review: &ChangeReview, width: u16) -> String {
    if review.detached {
        return "Detached HEAD".into();
    }
    let Some(upstream) = &review.upstream else {
        return "No upstream configured".into();
    };
    let branch = visible(upstream);
    if review.upstream_gone {
        format!("Upstream gone · {branch}")
    } else if review.ahead == 0 && review.behind == 0 {
        format!("Matches {branch}")
    } else {
        let full = format!(
            "{} ahead · {} behind vs {branch}",
            review.ahead, review.behind
        );
        if full.width() <= usize::from(width) {
            full
        } else {
            format!("↑{} ↓{} vs {branch}", review.ahead, review.behind)
        }
    }
}

fn clipped(text: &str, width: u16) -> String {
    let text = visible(text);
    if text.width() <= usize::from(width) {
        return text.into_owned();
    }
    let mut value = String::new();
    let mut used = 1;
    for ch in text.chars() {
        used += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used > usize::from(width) {
            break;
        }
        value.push(ch);
    }
    if width > 0 {
        value.push('…');
    }
    value
}

fn action(review: &ChangeReview, width: u16) -> &'static str {
    match (review.history_patch_open, width >= 30) {
        (true, true) => "‹ Back · Esc",
        (true, false) => "‹ Back",
        (false, true) => "Open · Enter",
        (false, false) => "Open",
    }
}

pub(super) fn click(review: &mut ChangeReview, area: Rect, point: Position) -> bool {
    let footer = layout(area, review).footer;
    let label = action(review, footer.width);
    if !review.commits.is_empty()
        && footer.contains(point)
        && point.x - footer.x < label.width() as u16
    {
        review.history_patch_open = !review.history_patch_open;
        true
    } else {
        false
    }
}

pub(super) fn render(frame: &mut Frame, area: Rect, review: &ChangeReview, theme: &Theme) {
    let r = layout(area, review);
    if !review.history_patch_open {
        frame.render_widget(
            Block::default().style(Style::default().bg(theme.list_background())),
            r.list_area(),
        );
    }
    let muted = theme.status_idle_style();
    let title = Style::default()
        .fg(theme.title)
        .add_modifier(Modifier::BOLD);
    if review.commits.is_empty() {
        if let Some(message) = review.message.as_deref() {
            frame.render_widget(Paragraph::new(visible(message)).style(muted), r.summary);
            return;
        }
    }
    let status = if review.unborn {
        "No commits yet"
    } else {
        "Working tree clean"
    };
    if let Some(commit) = review
        .commits
        .get(review.selected)
        .filter(|_| review.history_patch_open)
    {
        frame.render_widget(
            Paragraph::new(clipped(&commit.subject, r.summary.width)).style(title),
            r.summary,
        );
    } else {
        frame.render_widget(
            Paragraph::new(status).style(if review.unborn {
                muted
            } else {
                Style::default().fg(theme.success)
            }),
            r.summary,
        );
    }
    frame.render_widget(
        Paragraph::new(clipped(
            &tracking(review, r.tracking.width),
            r.tracking.width,
        ))
        .style(if review.upstream_gone || review.behind > 0 {
            Style::default().fg(theme.warning)
        } else if review.upstream.is_some() && !review.detached {
            Style::default().fg(theme.hl.link)
        } else {
            muted
        }),
        r.tracking,
    );
    if review.commits.is_empty() {
        return;
    }
    frame.render_widget(Paragraph::new("Recent commits").style(title), r.heading);
    let first = first_row(review, r.commits.height);
    for (i, commit) in review
        .commits
        .iter()
        .enumerate()
        .skip(first)
        .take(r.commits.height.into())
    {
        let selected = i == review.selected;
        let style = if selected {
            Style::default().bg(theme.selection_bg)
        } else {
            Style::default()
        };
        let hash: String = commit.id.chars().take(7).collect();
        let line = Line::from(vec![
            Span::styled(
                if selected { "› " } else { "  " },
                style.fg(theme.title_focused),
            ),
            Span::styled(format!("{hash} "), style.fg(theme.hl.number)),
            Span::styled(
                clipped(&commit.subject, r.commits.width.saturating_sub(10)),
                style.fg(theme.foreground),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(
                r.commits.x,
                r.commits.y + (i - first) as u16,
                r.commits.width,
                1,
            ),
        );
    }
    if let Some(commit) = review.commits.get(review.selected) {
        let id: String = commit.id.chars().take(7).collect();
        let lead = if review.history_patch_open {
            Span::styled(id, Style::default().fg(theme.hl.number))
        } else {
            Span::styled("Patch", title)
        };
        let date = visible(&commit.date);
        let remaining = usize::from(r.detail.width).saturating_sub(lead.width() + 6 + date.width());
        let info = Line::from(vec![
            lead,
            Span::styled(" · ", muted),
            Span::styled(date, muted),
            Span::styled(" · ", muted),
            Span::styled(
                clipped(&commit.author, remaining as u16),
                Style::default().fg(theme.hl.r#type),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(info).style(Style::default().bg(theme.background)),
            r.detail,
        );
        if review.preview_commit.as_deref() != Some(commit.id.as_str()) || review.message.is_some()
        {
            frame.render_widget(
                Paragraph::new(visible(
                    review.message.as_deref().unwrap_or("Loading commit…"),
                ))
                .style(muted.bg(theme.background)),
                r.patch,
            );
        } else {
            paint_patch(frame, review, r.patch, theme);
        }
    }
    let label = action(review, r.footer.width);
    let mut hints = vec![Span::styled(label, Style::default().fg(theme.accent))];
    let mut used = label.width();
    let options = if review.history_patch_open {
        ["  ↑↓ scroll", "  PgUp/PgDn page", "  r refresh"]
    } else {
        ["  ↑↓ commit", "  PgUp/PgDn patch", "  r refresh"]
    };
    for hint in options {
        if used + hint.width() <= usize::from(r.footer.width) {
            hints.push(Span::styled(hint, muted));
            used += hint.width();
        }
    }
    frame.render_widget(Paragraph::new(Line::from(hints)), r.footer);
}

#[cfg(test)]
#[path = "../../tests/change_review_history.rs"]
mod tests;
