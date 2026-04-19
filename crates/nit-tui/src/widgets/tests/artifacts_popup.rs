//! Tests for the artifacts popup markdown renderer and swarm task line
//! builder. Each test asserts against the stringified spans emitted for
//! markdown, JSON, or equation fixtures so styling regressions are easy to
//! isolate from content regressions.

use super::{build_swarm_task_lines, render_markdown_document};
use crate::swarm::{SwarmPersistenceView, SwarmTaskArtifacts, SwarmTaskPersistenceView};
use crate::theme::Theme;
use nit_syntax::HighlightGroup;
use ratatui::text::Line;

/// Concatenate all span contents of a rendered line into a plain string.
/// Used for `contains()` / `any()` assertions against the rendered output.
fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("")
}

/// Render a markdown fragment at `width` and flatten each line to a plain
/// string — the shape every `render_markdown_document_*` test asserts against.
fn rendered_text(source: &str, theme: &Theme, width: usize) -> Vec<String> {
    render_markdown_document(source, theme, width)
        .iter()
        .map(line_text)
        .collect()
}

#[test]
fn render_markdown_document_formats_headings_lists_code_and_tables() {
    let theme = Theme::default();
    let text = rendered_text(
        "# Findings\n**Risks**\n- first item\n1. second item\n> quoted text\n```rust\nlet total = 42;\n```\n| Name | Value |\n| --- | --- |\n| path | docs/ANTIGRAVITY.md |\n",
        &theme,
        80,
    );

    assert!(
        text.iter().any(|line| line.contains("§ Findings")),
        "{text:?}"
    );
    assert!(text.iter().any(|line| line.contains("• Risks")), "{text:?}");
    assert!(
        text.iter().any(|line| line.contains(" - first item")),
        "{text:?}"
    );
    assert!(
        text.iter().any(|line| line.contains(" 1. second item")),
        "{text:?}"
    );
    assert!(
        text.iter().any(|line| line.contains("│ quoted text")),
        "{text:?}"
    );
    assert!(
        text.iter().any(|line| line.contains("code block (rust)")),
        "{text:?}"
    );
    assert!(
        text.iter().any(|line| line.contains("let total = 42;")),
        "{text:?}"
    );
    assert!(text.iter().any(|line| line.contains("| Name")), "{text:?}");
    assert!(
        text.iter()
            .any(|line| line.contains("| path") && line.contains("ANTIGRAVITY")),
        "{text:?}"
    );
}

#[test]
fn render_markdown_document_formats_json_and_math() {
    let theme = Theme::default();
    let text = rendered_text(
        "Inline math $E = mc^2$ stays visible.\n```json\n{\"enabled\":true,\"count\":2,\"items\":[1,2]}\n```\n$$\n\\int_0^1 x^2 dx = 1/3\n$$\n",
        &theme,
        80,
    );

    assert!(
        text.iter().any(|line| line.contains("code block (json)")),
        "{text:?}"
    );
    assert!(text.iter().any(|line| line.contains("│ {")), "{text:?}");
    assert!(
        text.iter().any(|line| line.contains("\"enabled\": true")),
        "{text:?}"
    );
    assert!(
        text.iter().any(|line| line.contains("\"items\": [")),
        "{text:?}"
    );
    assert!(
        text.iter().any(|line| line.contains("equation")),
        "{text:?}"
    );
    assert!(
        text.iter()
            .any(|line| line.contains("\\int_0^1 x^2 dx = 1/3")),
        "{text:?}"
    );
    assert!(
        text.iter().any(|line| line.contains("$E = mc^2$")),
        "{text:?}"
    );
}

#[test]
fn render_markdown_document_formats_raw_json_document() {
    let theme = Theme::default();
    let text = rendered_text("{\"name\":\"nit\",\"ok\":true}", &theme, 80);

    assert!(
        text.iter().any(|line| line.contains("json document")),
        "{text:?}"
    );
    assert!(
        text.iter().any(|line| line.contains("\"name\": \"nit\"")),
        "{text:?}"
    );
    assert!(
        text.iter().any(|line| line.contains("\"ok\": true")),
        "{text:?}"
    );
}

#[test]
fn render_markdown_document_uses_builtin_syntax_highlighting_for_rust_blocks() {
    let theme = Theme::default();
    let rendered = render_markdown_document("```rust\nlet value = 42;\n```", &theme, 80);
    let keyword_fg = theme.highlight_style(HighlightGroup::Keyword).fg;
    let number_fg = theme.highlight_style(HighlightGroup::Number).fg;

    let let_span = rendered
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "let")
        .expect("keyword span");
    let number_span = rendered
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "42")
        .expect("number span");

    assert_eq!(let_span.style.fg, keyword_fg);
    assert_eq!(number_span.style.fg, number_fg);
}

#[test]
fn build_swarm_task_lines_renders_raw_markdown_output() {
    let theme = Theme::default();
    let view = SwarmPersistenceView {
        mission_id: "mis-001".into(),
        template: "rust-ci".into(),
        phase: "EXEC".into(),
        gate_bundle: None,
        gate_selection: String::new(),
        gate_report: None,
        gate_output: None,
        report_status: None,
        report_agent_id: None,
        report_output: None,
        tasks: Vec::new(),
    };
    let task = SwarmTaskPersistenceView {
        id: "source-map".into(),
        title: "Map source-backed ideas".into(),
        role: Some("research".into()),
        agent_id: "agent-1".into(),
        state: "DONE".into(),
        deps: Vec::new(),
        blocked_on: Vec::new(),
        writes: true,
        done_when: Some("recommendation list ready".into()),
        expected_artifacts: vec!["output.md".into()],
        expected_artifacts_missing: false,
        output_present: true,
        output: Some("**Findings**\n- source-backed item\n".into()),
        artifacts: Some(SwarmTaskArtifacts {
            summary: Some("captured summary".into()),
            ..SwarmTaskArtifacts::default()
        }),
    };

    let rendered = build_swarm_task_lines(&view, &task, &theme, 80);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert!(
        text.iter().any(|line| line.contains(" Document")),
        "{text:?}"
    );
    assert!(
        text.iter()
            .any(|line| line.contains(".nit/swarm/mis-001/tasks/source-map/output.md")),
        "{text:?}"
    );
    assert!(
        text.iter().any(|line| line.contains("• Findings")),
        "{text:?}"
    );
    assert!(
        text.iter()
            .any(|line| line.contains(" - source-backed item")),
        "{text:?}"
    );
}
