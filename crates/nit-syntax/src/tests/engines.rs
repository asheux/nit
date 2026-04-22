use crate::engine::tree_sitter::TreeSitterEngine;
use crate::{HighlightGroup, LanguageId, SyntaxEngine};

use super::{has_group, make_request, wait_for};

// One parameterized smoke test replaces the per-language keyword assertions:
// every grammar in the table should produce at least one Keyword span on the
// opening line of a minimal program.
#[test]
fn highlights_keywords_per_language() {
    let cases: &[(usize, LanguageId, &str)] = &[
        (1, LanguageId::Rust, "fn main() { let x = 42; }\n"),
        (2, LanguageId::Python, "def foo(x):\n    return x\n"),
        (4, LanguageId::JavaScript, "function foo() { return 1; }\n"),
    ];

    for &(buffer_id, lang, src) in cases {
        let mut engine = TreeSitterEngine::new();
        engine.schedule_rehighlight(make_request(buffer_id, 1, lang, src));
        let snap = wait_for(&mut engine, buffer_id, 1);
        assert!(
            has_group(&snap, 0, HighlightGroup::Keyword),
            "{lang:?} did not highlight a Keyword on line 0"
        );
    }
}

#[test]
fn rust_highlights_numbers() {
    let mut engine = TreeSitterEngine::new();
    let req = make_request(1, 1, LanguageId::Rust, "fn main() { let x = 42; }\n");
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 1, 1);
    assert!(has_group(&snap, 0, HighlightGroup::Number));
}

#[test]
fn markdown_highlights_heading() {
    let mut engine = TreeSitterEngine::new();
    let req = make_request(3, 1, LanguageId::Markdown, "# Title\n\nText\n");
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 3, 1);
    assert!(has_group(&snap, 0, HighlightGroup::Heading));
}

#[test]
fn language_change_invalidates_cache() {
    let mut engine = TreeSitterEngine::new();
    let text = "fn main() {}\n";

    engine.schedule_rehighlight(make_request(40, 1, LanguageId::Rust, text));
    let snap1 = wait_for(&mut engine, 40, 1);
    assert_eq!(snap1.language, LanguageId::Rust);

    engine.schedule_rehighlight(make_request(40, 2, LanguageId::Python, text));
    let snap2 = wait_for(&mut engine, 40, 2);
    assert_eq!(snap2.language, LanguageId::Python);
}

#[test]
fn worker_handles_plaintext_then_real_language() {
    let mut engine = TreeSitterEngine::new();

    engine.schedule_rehighlight(make_request(50, 1, LanguageId::PlainText, "hello\n"));
    let snap = wait_for(&mut engine, 50, 1);
    assert_eq!(snap.buffer_id, 50);

    engine.schedule_rehighlight(make_request(51, 1, LanguageId::Rust, "let x = 1;\n"));
    let snap = wait_for(&mut engine, 51, 1);
    assert!(has_group(&snap, 0, HighlightGroup::Keyword));
}

#[test]
fn highlighted_range_none_for_eager_mode() {
    let mut engine = TreeSitterEngine::new();
    let req = make_request(60, 1, LanguageId::Rust, "fn main() {}\n");
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 60, 1);
    assert!(
        snap.highlighted_range.is_none(),
        "eager mode should have highlighted_range = None"
    );
}
