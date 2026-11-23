use crate::engine::tree_sitter::TreeSitterEngine;
use crate::tests::{
    has_group, make_request, make_viewport_request, poll_until, wait_for, LONG_TIMEOUT,
};
use crate::{HighlightGroup as Group, LanguageId, SyntaxEngine, ViewportRange};

#[test]
fn markdown_inline_and_fenced_content_survive_an_edit() {
    let mut engine = TreeSitterEngine::new();
    let src = "*word* and [guide](route) with `code`\n\n```javascript\nconst n = 42;\n```\n";
    for version in 1..=2 {
        let text = if version == 1 {
            src.to_string()
        } else {
            src.replacen("word", "term", 1)
        };
        let mut request = make_request(311, version, LanguageId::Markdown, &text);
        if version > 1 {
            request.full_reparse = false;
            let point = |column| nit_core::BufferPoint { row: 0, column };
            request.edits.push(nit_core::BufferEdit {
                start_byte: 1,
                old_end_byte: 5,
                new_end_byte: 5,
                start_point: point(1),
                old_end_point: point(5),
                new_end_point: point(5),
            });
        }
        engine.schedule_rehighlight(request);
        let snap = wait_for(&mut engine, 311, version);
        for (line, group) in [
            (0, Group::Emphasis),
            (0, Group::Link),
            (0, Group::String),
            (3, Group::Number),
        ] {
            assert!(
                has_group(&snap, line, group),
                "{group:?} missing at version {version}: {:?}",
                snap.per_line[line]
            );
        }
    }
}

#[test]
fn viewport_and_background_fill_preserve_multiline_injections() {
    let mut engine = TreeSitterEngine::new();
    for (id, language, start, end, padding, code) in [
        (
            312,
            LanguageId::Html,
            "<script>\n",
            "</script>\n",
            "<div>word</div>\n",
            "const n = 42;\n",
        ),
        (
            313,
            LanguageId::Markdown,
            "```javascript\n",
            "```\n",
            "*word*\n\n",
            "const n = 42;\n",
        ),
    ] {
        let prefix = padding.repeat(240);
        let first_code = prefix.lines().count() + 1;
        let text = format!(
            "{prefix}{start}{}{end}{prefix}{start}{code}{end}",
            code.repeat(450)
        );
        let end_code = text.lines().count() - 2;
        engine.schedule_rehighlight(make_viewport_request(
            id,
            1,
            language,
            &text,
            ViewportRange {
                first_line: first_code + 220,
                last_line: first_code + 223,
                total_lines: text.lines().count(),
            },
        ));
        let first = wait_for(&mut engine, id, 1);
        assert!(has_group(&first, first_code + 220, Group::Number));
        let final_view = poll_until(
            &mut engine,
            id,
            1,
            |snapshot| snapshot.highlighted_range.is_none(),
            LONG_TIMEOUT,
        );
        assert!(has_group(&final_view, end_code, Group::Number));
    }
}
