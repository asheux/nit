use ratatui::text::Line;

use crate::swarm::{SwarmPersistenceView, SwarmTaskArtifacts, SwarmTaskPersistenceView};
use crate::theme::Theme;

use super::super::{
    popup_note_line, popup_rule_line, popup_section_line, popup_title_line, push_wrapped_bullet,
    push_wrapped_detail_lines, render_markdown_document,
};

/// TASK card — agent metadata, expected artifacts checklist, captured
/// artifacts (when the proposer/judge produced a `swarm_artifacts` JSON
/// block), and the markdown-rendered task output document.
pub(crate) fn build_swarm_task_lines(
    view: &SwarmPersistenceView,
    task: &SwarmTaskPersistenceView,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    out.push(popup_title_line(
        &format!(" TASK  {}  {}", task.agent_id, task.state),
        theme,
    ));
    out.push(popup_rule_line(width, theme));
    push_wrapped_detail_lines(
        &mut out,
        "task",
        &format!("{}  {}", task.id, task.title),
        theme,
        width,
    );
    push_wrapped_detail_lines(&mut out, "mission", &view.mission_id, theme, width);
    if let Some(role) = task.role.as_deref().filter(|role| !role.trim().is_empty()) {
        push_wrapped_detail_lines(&mut out, "role", role, theme, width);
    }
    push_wrapped_detail_lines(
        &mut out,
        "writes",
        if task.writes { "yes" } else { "no" },
        theme,
        width,
    );
    if let Some(done_when) = task
        .done_when
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        push_wrapped_detail_lines(&mut out, "done_when", done_when, theme, width);
    }
    if !task.deps.is_empty() {
        push_wrapped_detail_lines(&mut out, "deps", &task.deps.join(", "), theme, width);
    }
    if !task.blocked_on.is_empty() {
        push_wrapped_detail_lines(
            &mut out,
            "blocked_on",
            &task.blocked_on.join(", "),
            theme,
            width,
        );
    }
    if !task.expected_artifacts.is_empty() {
        push_wrapped_detail_lines(
            &mut out,
            "expected",
            &task.expected_artifacts.join(", "),
            theme,
            width,
        );
    }
    if task.expected_artifacts_missing {
        out.push(popup_note_line(
            " expected artifacts but no parseable swarm_artifacts JSON block was captured",
            theme.warning,
            theme,
        ));
    }
    push_wrapped_detail_lines(
        &mut out,
        "artifact",
        &format!(
            ".nit/swarm/{}/tasks/{}/artifacts.json",
            view.mission_id, task.id
        ),
        theme,
        width,
    );
    if let Some(artifacts) = task.artifacts.as_ref() {
        push_task_artifact_sections(&mut out, artifacts, theme, width);
    }

    out.push(Line::from(""));
    out.push(popup_section_line(" Document", theme));
    push_wrapped_detail_lines(
        &mut out,
        "output",
        &format!(".nit/swarm/{}/tasks/{}/output.md", view.mission_id, task.id),
        theme,
        width,
    );
    if let Some(output) = task.output.as_deref() {
        out.push(Line::from(""));
        out.extend(render_markdown_document(output, theme, width));
    } else {
        out.push(popup_note_line(
            " no captured task output",
            theme.border,
            theme,
        ));
    }
    out
}

fn push_task_artifact_sections(
    out: &mut Vec<Line<'static>>,
    artifacts: &SwarmTaskArtifacts,
    theme: &Theme,
    width: usize,
) {
    if artifacts.summary.is_none()
        && artifacts.files.is_empty()
        && artifacts.diffs.is_empty()
        && artifacts.commands.is_empty()
        && artifacts.risks.is_empty()
        && artifacts.notes.is_empty()
    {
        return;
    }

    out.push(Line::from(""));
    out.push(popup_section_line(" Artifacts", theme));
    if let Some(summary) = artifacts
        .summary
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        push_wrapped_detail_lines(out, "summary", summary, theme, width);
    }
    for file in artifacts.files.iter() {
        let text = match file.notes.as_deref().filter(|text| !text.trim().is_empty()) {
            Some(notes) => format!("{} ({})", file.path, notes.trim()),
            None => file.path.clone(),
        };
        push_wrapped_bullet(out, &text, theme, width);
    }
    for diff in artifacts.diffs.iter() {
        let text = match diff.path.as_deref().filter(|text| !text.trim().is_empty()) {
            Some(path) => format!("{} ({})", diff.summary, path.trim()),
            None => diff.summary.clone(),
        };
        push_wrapped_bullet(out, &text, theme, width);
    }
    for command in artifacts.commands.iter() {
        let text = match command
            .purpose
            .as_deref()
            .filter(|text| !text.trim().is_empty())
        {
            Some(purpose) => format!("{} ({})", command.cmd, purpose.trim()),
            None => command.cmd.clone(),
        };
        push_wrapped_bullet(out, &text, theme, width);
    }
    for risk in artifacts.risks.iter() {
        let prefix = risk
            .level
            .as_deref()
            .map(str::trim)
            .filter(|level| !level.is_empty())
            .map(|level| format!("[{level}] "))
            .unwrap_or_default();
        let text = match risk
            .mitigation
            .as_deref()
            .filter(|text| !text.trim().is_empty())
        {
            Some(mitigation) => format!("{prefix}{} -> {}", risk.item, mitigation.trim()),
            None => format!("{prefix}{}", risk.item),
        };
        push_wrapped_bullet(out, &text, theme, width);
    }
    for note in artifacts.notes.iter() {
        push_wrapped_bullet(out, note, theme, width);
    }
}
