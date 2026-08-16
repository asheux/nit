//! Phase 7 multiway routing: `mode=multiway` detection + classification across
//! @shadow / @swarm / @all / bare chat, the `@multiway-graph` / `@multiway-popup`
//! commands, and the source → policy mapping. The headline guarantee under test
//! is the byte-identical off-path: with `NIT_MULTIWAY` off — or on but with no
//! `mode=multiway` token — the chat submit records no multiway intent and the
//! prompt flows to the existing parsers exactly as before.

use super::*;

use crate::multiway::dispatch::{
    classify, extract_mode_multiway, parse_multiway_graph_command, parse_multiway_popup_command,
    resolve_multiway_policy,
};
use crate::multiway::runtime::clamp_fork_width;
use crate::multiway::{Mood, SearchPolicy};
use crate::swarm::effective_max_swarm_size;

fn agent_lane(id: &str) -> nit_core::AgentLane {
    nit_core::AgentLane {
        id: id.into(),
        role: "main".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    }
}

/// A chat-ready state: `NIT_MULTIWAY` set as requested, a small roster with one
/// agent selected so the off-path dispatch has a valid target (and so the full
/// submit never panics), and `input` staged on the chat line.
fn chat_state(multiway_enabled: bool, input: &str) -> AppState {
    let mut state = state_for_test();
    state.agents.multiway_enabled = multiway_enabled;
    state.agents.agents.push(agent_lane("a1"));
    state.agents.agents.push(agent_lane("a2"));
    state.agents.selected_agent = Some("a1".into());
    state.agents.chat_input = input.into();
    state.agents.chat_input_cursor = input.chars().count();
    state
}

fn submit(state: &mut AppState) -> bool {
    let mut vitals = crate::vitals::VitalsState::default();
    let mut swarm = crate::swarm::SwarmRuntime::default();
    let mut shadow = crate::shadow::ShadowRuntime::default();
    crate::app::submit_chat_input_and_dispatch(
        state,
        &mut vitals,
        None,
        None,
        &mut swarm,
        &mut shadow,
    )
}

#[test]
fn flag_off_never_routes_to_multiway() {
    // Every front-end carrying `mode=multiway`, plus the explicit form and the
    // 6a/6b commands: with the flag off the whole gate is skipped, so no intent
    // is recorded and the input flows on to the existing parsers unchanged.
    for input in [
        "@swarm 3 mode=multiway do the thing",
        "@shadow mode=multiway do the thing",
        "@all mode=multiway do the thing",
        "mode=multiway do the thing",
        "@multiway do the thing",
        "@multiway-graph",
        "@multiway-popup",
    ] {
        let mut state = chat_state(false, input);
        submit(&mut state);
        assert!(
            state.agents.pending_multiway.is_none(),
            "flag off must not route `{input}`"
        );
        assert!(!state.agents.pending_multiway_graph, "flag off, `{input}`");
        assert!(!state.agents.show_multiway_popup, "flag off, `{input}`");
    }
}

#[test]
fn off_path_is_byte_identical_across_front_ends() {
    // The byte-identical off-path, on every front-end. The gate stays shut in two
    // situations: the flag is off (even when `mode=multiway` is present), or the
    // flag is on but the token is an inexact near-miss `extract_mode_multiway` must
    // leave alone (`mode=linear`). In both, no intent is recorded and the `mode=`
    // token reaches the existing parsers verbatim in the dispatched operator
    // message — a partial strip would break byte-identity even though nothing routed.
    let cases = [
        (false, "@shadow mode=multiway do it", "mode=multiway"),
        (false, "@swarm 3 mode=multiway do it", "mode=multiway"),
        (false, "@all mode=multiway do it", "mode=multiway"),
        (false, "mode=multiway do it", "mode=multiway"),
        (true, "@shadow mode=linear do it", "mode=linear"),
        (true, "@swarm 3 mode=linear do it", "mode=linear"),
        (true, "@all mode=linear do it", "mode=linear"),
        (true, "mode=linear do it", "mode=linear"),
    ];
    for (enabled, input, literal) in cases {
        let mut state = chat_state(enabled, input);
        assert!(submit(&mut state));
        assert!(
            state.agents.pending_multiway.is_none(),
            "off-path must not route `{input}`"
        );
        let kept_literal = state
            .agents
            .messages
            .iter()
            .any(|message| message.text.contains(literal));
        assert!(
            kept_literal,
            "off-path must dispatch `{literal}` verbatim: `{input}`"
        );
    }
}

#[test]
fn swarm_mode_multiway_routes_with_cleaned_command() {
    let mut state = chat_state(true, "@swarm 4 mode=multiway build the parser");
    assert!(submit(&mut state));
    assert_eq!(
        state.agents.pending_multiway,
        Some(nit_core::PendingMultiway {
            source: nit_core::MultiwaySource::Swarm,
            command: "@swarm 4 build the parser".to_string(),
        })
    );
}

#[test]
fn swarm_mode_multiway_preserves_template_and_mission() {
    // Stripping the modifier must leave the `@swarm` size + `template=`/`mission=`
    // flags untouched so the run loop re-parses the cleaned command losslessly —
    // `mode=multiway` is the only token removed, regardless of where it sits.
    let mut state = chat_state(
        true,
        "@swarm 6 template=lab mission=research mode=multiway port the engine",
    );
    assert!(submit(&mut state));
    let pending = state
        .agents
        .pending_multiway
        .clone()
        .expect("routed to multiway");
    assert_eq!(pending.source, nit_core::MultiwaySource::Swarm);
    assert_eq!(
        pending.command,
        "@swarm 6 template=lab mission=research port the engine"
    );
    // The cleaned command still parses back with its flags intact.
    let cmd =
        crate::swarm::parse_swarm_command(&pending.command).expect("cleaned command re-parses");
    assert_eq!(cmd.template.as_deref(), Some("lab"));
    assert_eq!(cmd.prompt, "port the engine");
}

#[test]
fn shadow_mode_multiway_routes_with_cleaned_command() {
    let mut state = chat_state(true, "@shadow mode=multiway refactor the loop");
    assert!(submit(&mut state));
    assert_eq!(
        state.agents.pending_multiway,
        Some(nit_core::PendingMultiway {
            source: nit_core::MultiwaySource::Shadow,
            command: "@shadow refactor the loop".to_string(),
        })
    );
}

#[test]
fn all_mode_multiway_routes_with_cleaned_command() {
    let mut state = chat_state(true, "@all mode=multiway summarise the diff");
    assert!(submit(&mut state));
    assert_eq!(
        state.agents.pending_multiway,
        Some(nit_core::PendingMultiway {
            source: nit_core::MultiwaySource::All,
            command: "@all summarise the diff".to_string(),
        })
    );
}

#[test]
fn bare_mode_multiway_routes_as_bare_source() {
    let mut state = chat_state(true, "mode=multiway find the bug");
    assert!(submit(&mut state));
    assert_eq!(
        state.agents.pending_multiway,
        Some(nit_core::PendingMultiway {
            source: nit_core::MultiwaySource::Bare,
            command: "find the bug".to_string(),
        })
    );
}

#[test]
fn explicit_multiway_routes_as_explicit_source() {
    let mut state = chat_state(true, "@multiway mood=explore k=4 search wide");
    assert!(submit(&mut state));
    assert_eq!(
        state.agents.pending_multiway,
        Some(nit_core::PendingMultiway {
            source: nit_core::MultiwaySource::Explicit,
            command: "@multiway mood=explore k=4 search wide".to_string(),
        })
    );
}

#[test]
fn multiway_graph_command_sets_render_intent() {
    let mut state = chat_state(true, "@multiway-graph");
    assert!(submit(&mut state));
    assert!(state.agents.pending_multiway_graph);
    assert!(state.agents.pending_multiway.is_none());
}

#[test]
fn multiway_popup_command_toggles_flag() {
    let mut state = chat_state(true, "@multiway-popup");
    assert!(submit(&mut state));
    assert!(state.agents.show_multiway_popup);
    // The command is a toggle: a second submit closes it again.
    state.agents.chat_input = "@multiway-popup".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    assert!(submit(&mut state));
    assert!(!state.agents.show_multiway_popup);
}

#[test]
fn extract_mode_multiway_strips_one_token_and_preserves_the_rest() {
    assert_eq!(
        extract_mode_multiway("@swarm 4 mode=multiway do X"),
        (true, "@swarm 4 do X".to_string())
    );
    // Leading (bare) position, value matched case-insensitively.
    assert_eq!(
        extract_mode_multiway("mode=MULTIWAY fix it"),
        (true, "fix it".to_string())
    );
    // Trailing token with no body after it.
    assert_eq!(
        extract_mode_multiway("@shadow mode=multiway"),
        (true, "@shadow".to_string())
    );
    // A multi-line task body keeps its newline.
    assert_eq!(
        extract_mode_multiway("mode=multiway line one\nline two"),
        (true, "line one\nline two".to_string())
    );
    // No token, and a different `mode=` value, are both left untouched.
    assert_eq!(
        extract_mode_multiway("@swarm do X"),
        (false, "@swarm do X".to_string())
    );
    assert_eq!(
        extract_mode_multiway("@swarm mode=linear do X"),
        (false, "@swarm mode=linear do X".to_string())
    );
}

#[test]
fn classify_maps_each_prefix_to_its_source() {
    assert_eq!(
        classify("@shadow fix X"),
        Some(nit_core::MultiwaySource::Shadow)
    );
    assert_eq!(
        classify("@swarm 3 fix X"),
        Some(nit_core::MultiwaySource::Swarm)
    );
    assert_eq!(classify("@all fix X"), Some(nit_core::MultiwaySource::All));
    assert_eq!(classify("plain task"), Some(nit_core::MultiwaySource::Bare));
    // A recognised prefix with no body is not routable, and is NOT silently
    // reinterpreted as a bare-chat task.
    assert_eq!(classify("@shadow"), None);
    assert_eq!(classify("@all"), None);
    assert_eq!(classify("   "), None);
}

#[test]
fn graph_and_popup_commands_are_recognised() {
    assert!(parse_multiway_graph_command("@multiway-graph"));
    assert!(parse_multiway_graph_command("  @multiway-graph  "));
    assert!(!parse_multiway_graph_command("@multiway"));
    assert!(!parse_multiway_graph_command("@multiway-graphics"));
    assert!(parse_multiway_popup_command("@multiway-popup"));
    assert!(!parse_multiway_popup_command("@multiway-popups"));
}

#[test]
fn resolve_dispatches_each_source_to_its_mapper() {
    let cap = effective_max_swarm_size();

    // @shadow → k=2, Balanced.
    let (policy, task) =
        resolve_multiway_policy(nit_core::MultiwaySource::Shadow, "@shadow fix X", 0).unwrap();
    assert_eq!(policy.mood, Mood::Balanced);
    assert_eq!(policy.k, clamp_fork_width(2, cap));
    assert_eq!(task.prompt, "fix X");

    // @swarm size→k, template=parallel → Explore.
    let (policy, task) = resolve_multiway_policy(
        nit_core::MultiwaySource::Swarm,
        "@swarm 4 template=parallel do Y",
        0,
    )
    .unwrap();
    assert_eq!(policy.mood, Mood::Explore);
    assert_eq!(policy.k, clamp_fork_width(4, cap));
    assert_eq!(task.prompt, "do Y");

    // @all → Explore, k = fan_out.
    let (policy, task) =
        resolve_multiway_policy(nit_core::MultiwaySource::All, "@all broadcast", 5).unwrap();
    assert_eq!(policy.mood, Mood::Explore);
    assert_eq!(policy.k, clamp_fork_width(5, cap));
    assert_eq!(task.prompt, "broadcast");

    // Bare → policy defaults.
    let defaults = SearchPolicy::default();
    let (policy, task) =
        resolve_multiway_policy(nit_core::MultiwaySource::Bare, "just do it", 0).unwrap();
    assert_eq!(policy.mood, defaults.mood);
    assert_eq!(policy.k, defaults.k);
    assert_eq!(task.prompt, "just do it");

    // Explicit is resolved app-side (its `mood=`/`k=` parser lives there).
    assert!(
        resolve_multiway_policy(nit_core::MultiwaySource::Explicit, "@multiway X", 0).is_none()
    );
    // A cleaned command that no longer parses (empty body) is None.
    assert!(resolve_multiway_policy(nit_core::MultiwaySource::Shadow, "@shadow", 0).is_none());
}

#[test]
fn selector_mode_on_drives_resolved_search_policy() {
    // Phase 9: roster Mode=multiway + Mood=explore. A bare, token-less dispatch
    // routes through the engine and the resolved policy carries the selected mood.
    let mut state = chat_state(true, "build the parser");
    state.agents.multiway_default_mode_on = true;
    state.agents.multiway_default_mood = nit_core::MultiwaySearchMood::Explore;
    assert!(submit(&mut state));

    let pending = state
        .agents
        .pending_multiway
        .clone()
        .expect("selector routed the bare dispatch to multiway");
    assert_eq!(pending.source, nit_core::MultiwaySource::Bare);
    assert_eq!(pending.command, "build the parser");
    assert_eq!(
        state.agents.pending_multiway_mood,
        Some(nit_core::MultiwaySearchMood::Explore)
    );

    // The raw source→policy map (no selector) yields the Bare default, Balanced...
    let (raw_policy, _) =
        resolve_multiway_policy(pending.source, &pending.command, 0).expect("bare resolves");
    assert_eq!(raw_policy.mood, Mood::Balanced);
    // ...and the real dispatch applies the carried selector mood over it, so the policy
    // a search actually runs with carries Explore — proof the selector, not the
    // front-end default, drove the engine mood. Guards the runner-side carry directly.
    let (policy, task) =
        crate::app::runner::resolve_multiway_dispatch(&state, &pending).expect("dispatch resolves");
    assert_eq!(policy.mood, Mood::Explore);
    assert_eq!(task.prompt, "build the parser");
}

#[test]
fn typed_mood_overrides_selector_default() {
    // Even with the roster selecting Mode=multiway / Mood=explore, a typed
    // `@multiway mood=exploit` wins: the explicit branch records no per-dispatch
    // mood, so nothing overrides the typed `exploit` at resolution time.
    let mut state = chat_state(true, "@multiway mood=exploit search wide");
    state.agents.multiway_default_mode_on = true;
    state.agents.multiway_default_mood = nit_core::MultiwaySearchMood::Explore;
    assert!(submit(&mut state));

    let pending = state
        .agents
        .pending_multiway
        .clone()
        .expect("explicit @multiway still routes");
    assert_eq!(pending.source, nit_core::MultiwaySource::Explicit);
    // The selector did NOT inject its mood over the typed command.
    assert_eq!(state.agents.pending_multiway_mood, None);

    // The explicit command's own `mood=exploit` is what resolves; with no carried
    // selector mood there is nothing to override it back to the roster's `explore`.
    let command = crate::app::chat_input::parse_multiway_command(&pending.command)
        .expect("explicit command re-parses");
    assert_eq!(command.mood, Mood::Exploit);
}

#[test]
fn flag_off_selector_does_not_route() {
    // The Phase 9 selector is flag-gated exactly like the Phase 7 routing: with
    // `NIT_MULTIWAY` off, Mode=multiway selects nothing — the bare dispatch flows to
    // the normal parsers and records no multiway intent (off-path byte-identical).
    let mut state = chat_state(false, "build the parser");
    state.agents.multiway_default_mode_on = true;
    state.agents.multiway_default_mood = nit_core::MultiwaySearchMood::Explore;
    submit(&mut state);
    assert!(state.agents.pending_multiway.is_none());
    assert_eq!(state.agents.pending_multiway_mood, None);
}
