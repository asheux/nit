//! Operator-intent extraction from the raw `@swarm` prompt.
//!
//! The planner LLM has a strong consolidation prior — given a prompt with
//! nine bullets it will still try to produce a single integrator plan.
//! Extracting an explicit ticket count up front gives the planner prompt
//! a numeric `MUST` it can't ignore, and gives the post-parse validator
//! something concrete to enforce + auto-repair against.
//!
//! Heuristic, not perfect — false positives are cheap (push toward
//! fanout, which is the safe direction) and false negatives just fall
//! back to "ambiguous, planner picks". We never want to *block* on this,
//! only to nudge.

/// What the operator prompt seems to be asking for, in machine-readable
/// terms.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OperatorIntent {
    /// Best estimate of distinct deliverables in the prompt. `None`
    /// means the prompt is unstructured prose and the planner should
    /// fall back to its normal heuristic for that template.
    pub ticket_count: Option<usize>,
    /// `true` when the prompt enumerates items in an obviously
    /// structured way (≥ `MIN_LIST_ITEMS_FOR_INTENT` bullets, numbered
    /// items, or `T<n>.` ticket headers). Drives the "MUST emit N
    /// integrators" planner directive.
    pub structured_list: bool,
}

/// Below this count we treat the list signal as ambiguous (incidental
/// bullets in prose). Three is the threshold where the operator
/// almost certainly meant a list.
const MIN_LIST_ITEMS_FOR_INTENT: usize = 3;
/// Above this we assume the operator's count is approximate and clamp,
/// preventing a runaway prompt with 30 incidental bullets from
/// requesting 30 integrators.
const MAX_TICKET_COUNT: usize = 32;

pub fn detect_intent(prompt: &str) -> OperatorIntent {
    let bullet = count_bullets(prompt);
    let numbered = count_numbered_items(prompt);
    let t_header = count_ticket_headers(prompt);
    let files_block = count_files_blocks(prompt);

    // Pick the strongest signal — the count an operator would point to
    // when asked "how many distinct things did you ask for?". Ticket
    // headers (`T13.`) and `Files:` blocks are more reliable than raw
    // bullets, so they win when present.
    let raw_count = [t_header, files_block, numbered, bullet]
        .into_iter()
        .find(|c| *c >= MIN_LIST_ITEMS_FOR_INTENT)
        .unwrap_or(0);

    if raw_count < MIN_LIST_ITEMS_FOR_INTENT {
        return OperatorIntent::default();
    }
    OperatorIntent {
        ticket_count: Some(raw_count.min(MAX_TICKET_COUNT)),
        structured_list: true,
    }
}

fn count_bullets(prompt: &str) -> usize {
    prompt
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            // `- foo` (markdown), `* foo` (markdown alt), `• foo`
            // (typed bullet). Require at least one whitespace
            // following so we don't match `--flag` or `*ptr`.
            trimmed
                .strip_prefix(['-', '*', '•'])
                .is_some_and(|rest| rest.starts_with(char::is_whitespace))
        })
        .count()
}

fn count_numbered_items(prompt: &str) -> usize {
    prompt
        .lines()
        .filter(|line| {
            // `1.` or `1)` followed by whitespace, with the number
            // bounded to 1..=99 so a year ("2026.") doesn't false-match.
            let trimmed = line.trim_start();
            let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() || digits.len() > 2 {
                return false;
            }
            let rest = &trimmed[digits.len()..];
            rest.starts_with('.')
                || rest.starts_with(')') && rest[1..].starts_with(char::is_whitespace)
        })
        .count()
}

fn count_ticket_headers(prompt: &str) -> usize {
    prompt
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            let Some(rest) = trimmed.strip_prefix(['T', 't']) else {
                return false;
            };
            // `T13.` `T1.` `t22.` — digits then period.
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() || digits.len() > 3 {
                return false;
            }
            rest[digits.len()..].starts_with('.')
        })
        .count()
}

fn count_files_blocks(prompt: &str) -> usize {
    // Each "Files:" header (or "Files :" with stray space) typically
    // scopes one deliverable. Case-insensitive match, line-start
    // anchored (with optional whitespace).
    prompt
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            let lower = trimmed.to_ascii_lowercase();
            lower.starts_with("files:") || lower.starts_with("files :")
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_prompt_is_ambiguous() {
        let intent = detect_intent("");
        assert_eq!(intent.ticket_count, None);
        assert!(!intent.structured_list);
    }

    #[test]
    fn unstructured_prose_is_ambiguous() {
        let prompt = "Fix the editor so it does what vim does. The cursor \
                      should also move correctly when scrolling. Make sure \
                      undo works.";
        let intent = detect_intent(prompt);
        assert_eq!(intent.ticket_count, None);
        assert!(!intent.structured_list);
    }

    #[test]
    fn single_bullet_is_not_a_list() {
        let prompt = "Fix this:\n- the bracket highlight is missing.";
        // One bullet is below the threshold (could be incidental prose
        // formatting). Plan stays ambiguous.
        let intent = detect_intent(prompt);
        assert_eq!(intent.ticket_count, None);
    }

    #[test]
    fn three_bullets_count_as_structured() {
        let prompt = "Issues:\n\
                      - bracket highlight missing\n\
                      - % motion missing\n\
                      - jumplist unreliable\n";
        let intent = detect_intent(prompt);
        assert_eq!(intent.ticket_count, Some(3));
        assert!(intent.structured_list);
    }

    #[test]
    fn nine_bullets_yields_nine() {
        let prompt = "Tickets:\n\
                      - one\n- two\n- three\n- four\n- five\n\
                      - six\n- seven\n- eight\n- nine\n";
        let intent = detect_intent(prompt);
        assert_eq!(intent.ticket_count, Some(9));
    }

    #[test]
    fn numbered_list_counts() {
        let prompt = "1. first thing\n2. second thing\n3. third thing\n4. fourth\n";
        let intent = detect_intent(prompt);
        assert_eq!(intent.ticket_count, Some(4));
    }

    #[test]
    fn t_headers_count() {
        let prompt = "T13. bracket highlight\nT14. % motion\nT15. jumplist\n\
                      T16. preferred col\nT17. visual indent\n";
        let intent = detect_intent(prompt);
        assert_eq!(intent.ticket_count, Some(5));
    }

    #[test]
    fn ticket_headers_beat_bullets() {
        // Both signals present — ticket headers win because they're
        // more reliable. (5 T-headers, 3 bullets in the prose of those
        // tickets — only the headers should count.)
        let prompt = "\
T1. first ticket
  - some detail
T2. second ticket
  - another detail
T3. third ticket
  - more detail
T4. fourth ticket
T5. fifth ticket
";
        let intent = detect_intent(prompt);
        assert_eq!(intent.ticket_count, Some(5));
    }

    #[test]
    fn files_blocks_count() {
        let prompt = "\
Ticket A
Files:
- foo.rs
- bar.rs

Ticket B
Files:
- baz.rs

Ticket C
Files:
- qux.rs
";
        let intent = detect_intent(prompt);
        assert_eq!(intent.ticket_count, Some(3));
    }

    #[test]
    fn ticket_count_caps_at_max() {
        // Runaway bullet list should still produce a sane integer.
        let bullets = (0..50)
            .map(|i| format!("- item {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let intent = detect_intent(&bullets);
        assert_eq!(intent.ticket_count, Some(MAX_TICKET_COUNT));
    }

    #[test]
    fn flag_lines_not_counted_as_bullets() {
        // `--flag` looks like a dash but isn't a bullet. Make sure we
        // require whitespace after the dash.
        let prompt = "run with --verbose --foo --bar";
        let intent = detect_intent(prompt);
        assert_eq!(intent.ticket_count, None);
    }

    #[test]
    fn year_like_numbers_not_counted_as_list() {
        // `2026.` shouldn't false-match a numbered list. Three-digit+
        // numbers are rejected (max two digits for list items).
        let prompt = "2026. some date\n2027. another\n2028. last";
        let intent = detect_intent(prompt);
        // All three rejected → falls below threshold → ambiguous.
        assert_eq!(intent.ticket_count, None);
    }
}
