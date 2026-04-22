use crate::{FileClassification, HighlightOutcome, MAX_HIGHLIGHT_BYTES};

#[test]
fn file_classification_boundaries() {
    assert_eq!(
        FileClassification::from_byte_length(0),
        FileClassification::Empty
    );
    assert_eq!(
        FileClassification::from_byte_length(1),
        FileClassification::Normal
    );
    assert_eq!(
        FileClassification::from_byte_length(MAX_HIGHLIGHT_BYTES),
        FileClassification::Normal
    );
    assert_eq!(
        FileClassification::from_byte_length(MAX_HIGHLIGHT_BYTES + 1),
        FileClassification::Oversized
    );
}

#[test]
fn file_classification_expected_outcomes() {
    assert_eq!(
        FileClassification::Normal.expected_outcome(false),
        HighlightOutcome::Parsed
    );
    assert_eq!(
        FileClassification::Normal.expected_outcome(true),
        HighlightOutcome::ViewportOnly
    );
    assert_eq!(
        FileClassification::Oversized.expected_outcome(false),
        HighlightOutcome::PlainText
    );
    assert_eq!(
        FileClassification::Empty.expected_outcome(false),
        HighlightOutcome::PlainText
    );
}
