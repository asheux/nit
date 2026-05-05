use crate::{FileClassification, HighlightOutcome, MAX_HIGHLIGHT_BYTES};

#[test]
fn file_classification_boundaries() {
    let cases: &[(usize, FileClassification)] = &[
        (0, FileClassification::Empty),
        (1, FileClassification::Normal),
        (MAX_HIGHLIGHT_BYTES, FileClassification::Normal),
        (MAX_HIGHLIGHT_BYTES + 1, FileClassification::Oversized),
    ];
    for &(len, expected) in cases {
        assert_eq!(
            FileClassification::from_byte_length(len),
            expected,
            "byte_len={len}",
        );
    }
}

#[test]
fn file_classification_outcome_matrix() {
    // Full (variant, viewport_scoped) matrix. Oversized and Empty are
    // PlainText regardless of viewport scoping — the assertions document
    // that invariant rather than relying on impl inspection.
    let cases: &[(FileClassification, bool, HighlightOutcome)] = &[
        (FileClassification::Normal, false, HighlightOutcome::Parsed),
        (
            FileClassification::Normal,
            true,
            HighlightOutcome::ViewportOnly,
        ),
        (
            FileClassification::Oversized,
            false,
            HighlightOutcome::PlainText,
        ),
        (
            FileClassification::Oversized,
            true,
            HighlightOutcome::PlainText,
        ),
        (
            FileClassification::Empty,
            false,
            HighlightOutcome::PlainText,
        ),
        (FileClassification::Empty, true, HighlightOutcome::PlainText),
    ];
    for &(class, viewport_scoped, expected) in cases {
        assert_eq!(
            class.expected_outcome(viewport_scoped),
            expected,
            "{class:?} viewport_scoped={viewport_scoped}",
        );
    }
}
