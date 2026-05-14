use ratatui::text::Line;

use crate::swarm::SwarmPersistenceView;
use crate::theme::Theme;

use super::super::{
    popup_note_line, popup_rule_line, popup_section_line, popup_title_line, push_wrapped_bullet,
    push_wrapped_detail_lines, render_markdown_document,
};

/// FINAL synthesis view — the orchestrator's report document with mission
/// metadata and a markdown-rendered body.
pub(in crate::widgets::artifacts_popup) fn build_swarm_report_lines(
    view: &SwarmPersistenceView,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let status = view.report_status.as_deref().unwrap_or("FINAL");
    let agent_id = view.report_agent_id.as_deref().unwrap_or("planner");

    let mut out = Vec::new();
    out.push(popup_title_line(
        &format!(" REPORT  {agent_id}  {status}"),
        theme,
    ));
    out.push(popup_rule_line(width, theme));
    push_wrapped_detail_lines(&mut out, "mission", &view.mission_id, theme, width);
    push_wrapped_detail_lines(
        &mut out,
        "artifact",
        &format!(".nit/swarm/{}/report/final.md", view.mission_id),
        theme,
        width,
    );

    out.push(Line::from(""));
    out.push(popup_section_line(" Document", theme));
    if let Some(output) = view.report_output.as_deref() {
        out.push(Line::from(""));
        out.extend(render_markdown_document(output, theme, width));
    } else {
        out.push(popup_note_line(
            " no final synthesis output captured",
            theme.border,
            theme,
        ));
    }
    out
}

/// VERIFY view — gate-bundle status, per-gate PASS/FAIL bullets, and the
/// markdown-rendered verification document.
pub(in crate::widgets::artifacts_popup) fn build_swarm_verify_lines(
    view: &SwarmPersistenceView,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let status = if let Some(report) = view.gate_report.as_ref() {
        if report.overall_ok {
            "PASS"
        } else {
            "FAIL"
        }
    } else if view.gate_bundle.is_some() {
        "PENDING"
    } else {
        "--"
    };

    let mut out = Vec::new();
    out.push(popup_title_line(
        &format!(
            " VERIFY  {}  {status}",
            view.gate_bundle.as_deref().unwrap_or("none")
        ),
        theme,
    ));
    out.push(popup_rule_line(width, theme));
    push_wrapped_detail_lines(&mut out, "mission", &view.mission_id, theme, width);
    push_wrapped_detail_lines(&mut out, "template", &view.template, theme, width);
    push_wrapped_detail_lines(
        &mut out,
        "artifact",
        &format!(".nit/swarm/{}/gates/verify.md", view.mission_id),
        theme,
        width,
    );
    if view.gate_report.is_some() {
        push_wrapped_detail_lines(
            &mut out,
            "report",
            &format!(".nit/swarm/{}/gates/report.json", view.mission_id),
            theme,
            width,
        );
    }
    if view.gate_output.is_some() {
        push_wrapped_detail_lines(
            &mut out,
            "output",
            &format!(".nit/swarm/{}/gates/output.txt", view.mission_id),
            theme,
            width,
        );
    }

    if let Some(report) = view.gate_report.as_ref() {
        out.push(Line::from(""));
        out.push(popup_section_line(" Gates", theme));
        for gate in report.gates.iter() {
            push_wrapped_bullet(
                &mut out,
                &format!(
                    "{} [{}] {}",
                    gate.name,
                    if gate.ok { "PASS" } else { "FAIL" },
                    gate.command
                ),
                theme,
                width,
            );
            if let Some(notes) = gate.notes.as_deref().filter(|text| !text.trim().is_empty()) {
                push_wrapped_detail_lines(&mut out, "notes", notes, theme, width);
            }
        }
    }

    out.push(Line::from(""));
    out.push(popup_section_line(" Document", theme));
    if let Some(output) = view.gate_output.as_deref() {
        out.push(Line::from(""));
        out.extend(render_markdown_document(output, theme, width));
    } else if view.gate_bundle.is_some() {
        out.push(popup_note_line(
            " verification has not completed yet",
            theme.warning,
            theme,
        ));
    } else {
        out.push(popup_note_line(
            " no verification output captured",
            theme.border,
            theme,
        ));
    }
    out
}
