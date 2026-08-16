//! Phase 7 chat-routing for the multiway engine: detect the `mode=multiway`
//! modifier, classify which front-end the operator used, and map a cleaned
//! command to a [`SearchPolicy`]/[`Task`]. The chat submit calls
//! [`extract_mode_multiway`] + [`classify`] to record a [`PendingMultiway`]
//! intent; the run loop later calls [`resolve_multiway_policy`] to turn that
//! intent into a search.
//!
//! The per-front-end policy mappers live beside the command struct they read
//! (`shadow_multiway_policy`, `swarm_multiway_policy`); `@all` has no command
//! struct — just a fan-out count plus a body — so [`all_multiway_policy`] lives
//! here, and [`resolve_multiway_policy`] dispatches to all three by source.
//!
//! [`PendingMultiway`]: nit_core::PendingMultiway

use nit_core::{MultiwaySearchMood, MultiwaySource};

use super::runtime::clamp_fork_width;
use super::{Budget, Mood, SearchPolicy, Task};
use crate::shadow::{parse_shadow_command, shadow_multiway_policy};
use crate::swarm::{effective_max_swarm_size, parse_swarm_command, swarm_multiway_policy};

/// Strip the first whitespace-delimited `mode=multiway` token from `raw`,
/// returning `(found, cleaned)`. The value is matched case-insensitively; the
/// rest of the string is preserved byte-for-byte (only one adjacent separator is
/// collapsed) so the cleaned command re-parses losslessly through its front-end's
/// own parser — a multi-line task body keeps its newlines.
///
/// Called only when `multiway_enabled`, so when the flag is off `raw` is never
/// touched and the dispatch path stays byte-identical.
pub fn extract_mode_multiway(raw: &str) -> (bool, String) {
    for (start, _) in raw.match_indices("mode=") {
        let at_boundary = start == 0 || raw[..start].ends_with(char::is_whitespace);
        if !at_boundary {
            continue;
        }
        let end = raw[start..]
            .find(char::is_whitespace)
            .map_or(raw.len(), |off| start + off);
        if !raw[start..end].eq_ignore_ascii_case("mode=multiway") {
            continue;
        }
        let head = raw[..start].trim_end_matches(char::is_whitespace);
        let tail = raw[end..].trim_start_matches(char::is_whitespace);
        let cleaned = match (head.is_empty(), tail.is_empty()) {
            (true, _) => tail.to_owned(),
            (false, true) => head.to_owned(),
            (false, false) => format!("{head} {tail}"),
        };
        return (true, cleaned);
    }
    (false, raw.to_owned())
}

/// Classify which front-end a `mode=multiway`-cleaned command came from, or
/// `None` when it isn't routable (empty, or a recognised prefix with no body —
/// e.g. a bare `@shadow`). A command prefix that fails its own parser is *not*
/// reinterpreted as bare chat; only prefix-free text becomes [`MultiwaySource::Bare`].
pub fn classify(cleaned: &str) -> Option<MultiwaySource> {
    if cleaned.trim().is_empty() {
        return None;
    }
    if starts_with_command_word(cleaned, "@shadow") {
        return parse_shadow_command(cleaned).map(|_| MultiwaySource::Shadow);
    }
    if starts_with_command_word(cleaned, "@swarm") {
        return parse_swarm_command(cleaned).map(|_| MultiwaySource::Swarm);
    }
    if starts_with_command_word(cleaned, "@all") {
        return all_command_body(cleaned).map(|_| MultiwaySource::All);
    }
    Some(MultiwaySource::Bare)
}

/// `@multiway-graph`: render the current DAG to an image on demand (Phase 6a).
pub fn parse_multiway_graph_command(raw: &str) -> bool {
    starts_with_command_word(raw.trim(), "@multiway-graph")
}

/// `@multiway-popup`: toggle the live search popup (Phase 6b).
pub fn parse_multiway_popup_command(raw: &str) -> bool {
    starts_with_command_word(raw.trim(), "@multiway-popup")
}

/// Whether `s` begins with `word` as a whole token — `word` followed by
/// whitespace or end-of-string, so `@all` matches but `@allotment` does not.
fn starts_with_command_word(s: &str, word: &str) -> bool {
    s.strip_prefix(word)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// The prompt body of an `@all <body>` command, or `None` for a bare/empty `@all`.
fn all_command_body(command: &str) -> Option<&str> {
    let body = command.trim_start().strip_prefix("@all")?.trim();
    (!body.is_empty()).then_some(body)
}

/// Map an `@all mode=multiway <body>` dispatch to a single wide expansion.
///
/// `@all` broadcasts one prompt to a fan-out of agents and compares their
/// independent results; the multiway analogue is one expansion of width
/// `k = fan_out` whose children are valued and value-ranked on the frontier, the
/// best reported — `Explore` keeps the whole fan-out visible rather than pruning
/// to a single line. `k` is clamped to the FD-bounded swarm ceiling with the same
/// [`clamp_fork_width`] [`MultiwayRuntime::start`] applies, so the bound holds even
/// if a caller reads the policy before `start`.
///
/// [`MultiwayRuntime::start`]: super::MultiwayRuntime::start
pub fn all_multiway_policy(body: &str, fan_out: usize) -> (SearchPolicy, Task) {
    let k = clamp_fork_width(fan_out, effective_max_swarm_size());
    let policy = SearchPolicy {
        mood: Mood::Explore,
        k,
        budget: Budget {
            // One broadcast round, not a deep search: each of the k forks spends a
            // turn, so a turn budget of k stops the loop the first time it is
            // re-checked — right after the seed expansion. The node cap leaves room
            // for the root, the k children, and the Explore merge of the best pair.
            max_turns: k,
            max_nodes: k.saturating_add(2),
            max_tokens: None,
        },
    };
    let task = Task {
        prompt: body.to_owned(),
        role: "integrate".to_owned(),
    };
    (policy, task)
}

/// Map a routed [`MultiwaySource`] + cleaned command to a `(SearchPolicy, Task)`,
/// re-parsing the command through that front-end's own parser and applying its
/// mapper. Returns `None` when the cleaned command no longer parses (an empty
/// body). [`MultiwaySource::Explicit`] is resolved by the run loop in the app
/// layer (where `@multiway`'s `mood=`/`k=` parser lives), so it maps to `None`
/// here; `@all`'s fork width is the broadcast `fan_out` the caller supplies.
pub fn resolve_multiway_policy(
    source: MultiwaySource,
    command: &str,
    fan_out: usize,
) -> Option<(SearchPolicy, Task)> {
    match source {
        MultiwaySource::Shadow => Some(shadow_multiway_policy(&parse_shadow_command(command)?)),
        MultiwaySource::Swarm => Some(swarm_multiway_policy(&parse_swarm_command(command)?)),
        MultiwaySource::All => Some(all_multiway_policy(all_command_body(command)?, fan_out)),
        MultiwaySource::Bare => {
            let body = command.trim();
            (!body.is_empty()).then(|| {
                (
                    SearchPolicy::default(),
                    Task {
                        prompt: body.to_owned(),
                        role: "integrate".to_owned(),
                    },
                )
            })
        }
        MultiwaySource::Explicit => None,
    }
}

/// Project the roster's [`MultiwaySearchMood`] onto the engine [`Mood`]. nit-core
/// cannot name `nit_multiway::policy::Mood` (the crate edge runs the other way),
/// so this nit-tui-boundary map — the twin of how `MultiwayNodeStatus` mirrors the
/// node status — is where the Phase 9 selector mood crosses back into the engine.
///
/// [`MultiwaySearchMood`]: nit_core::MultiwaySearchMood
pub fn engine_mood(mood: MultiwaySearchMood) -> Mood {
    match mood {
        MultiwaySearchMood::Explore => Mood::Explore,
        MultiwaySearchMood::Balanced => Mood::Balanced,
        MultiwaySearchMood::Exploit => Mood::Exploit,
    }
}
