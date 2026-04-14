use crate::agents::{
    parse_claude_models_from_binary, parse_gemini_models_from_source, select_current_claude_models,
    select_current_gemini_models, sync_backend_model_lanes,
};
use crate::cli::AgentsArg;

fn test_lane(id: &str, role: &str, kind: nit_core::AgentLaneKind) -> nit_core::AgentLane {
    nit_core::AgentLane {
        id: id.into(),
        role: role.into(),
        lane: role.into(),
        kind,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    }
}

#[test]
fn parses_gemini_models_from_backend_source() {
    let typescript_source = r#"
        export const PREVIEW_GEMINI_MODEL = 'gemini-3-pro-preview';
        export const DEFAULT_GEMINI_MODEL = 'gemini-2.5-pro';
        export const DEFAULT_GEMINI_FLASH_MODEL = 'gemini-2.5-flash';
        export const VALID_GEMINI_MODELS = new Set([
            PREVIEW_GEMINI_MODEL,
            DEFAULT_GEMINI_MODEL,
            DEFAULT_GEMINI_FLASH_MODEL,
        ]);
    "#;

    assert_eq!(
        parse_gemini_models_from_source(typescript_source),
        ["gemini-2.5-flash", "gemini-2.5-pro", "gemini-3-pro-preview"]
    );
}

#[test]
fn keeps_only_current_gemini_models_by_family() {
    let gemini_candidates = [
        "gemini-3-pro-preview",
        "gemini-3.1-pro-preview-customtools",
        "gemini-2.5-pro",
        "gemini-3-flash-preview",
        "gemini-2.5-flash",
        "gemini-2.5-flash-lite",
    ]
    .map(String::from)
    .to_vec();

    let filtered = select_current_gemini_models(gemini_candidates);
    assert_eq!(
        filtered,
        [
            "gemini-2.5-flash",
            "gemini-2.5-flash-lite",
            "gemini-2.5-pro"
        ]
    );
}

#[test]
fn parses_claude_models_from_backend_binary_strings() {
    let binary_blob = br#"
        foundry
        claude-opus-4-6[1m]
        Opus 4.6 (with 1M context)
        claude-opus-4-6
        Opus 4.6
        claude-opus-4-5
        Opus 4.5
        claude-sonnet-4-6[1m]
        Sonnet 4.6 (with 1M context)
        claude-sonnet-4-6
        Sonnet 4.6
        claude-sonnet-4
        Sonnet 4
        claude-3-7-sonnet
        Claude 3.7 Sonnet
        claude-haiku-4-5
        Haiku 4.5
        haiku45
        sonnet46
        claude-3-5-haiku-20241022
        claude-sonnet-4-20250514
        claude-sonnet-4-latest
        claude-sonnet-4-v2
        claude-code
        claude-plugin-directory
    "#;

    let extracted = parse_claude_models_from_binary(binary_blob);
    assert_eq!(
        extracted,
        [
            "claude-3-7-sonnet",
            "claude-haiku-4-5",
            "claude-opus-4-5",
            "claude-opus-4-6",
            "claude-sonnet-4",
            "claude-sonnet-4-6",
        ]
    );
}

#[test]
fn keeps_only_current_claude_models_by_family() {
    let latest = select_current_claude_models(vec![
        "claude-3-5-haiku".into(),
        "claude-haiku-4-5".into(),
        "claude-3-7-sonnet".into(),
        "claude-sonnet-4".into(),
        "claude-sonnet-4-6".into(),
        "claude-opus-4-5".into(),
        "claude-opus-4-6".into(),
    ]);
    assert_eq!(
        latest,
        ["claude-haiku-4-5", "claude-opus-4-6", "claude-sonnet-4-6"]
    );
}

#[test]
fn sync_backend_model_lanes_replaces_placeholder_backend_rows() {
    let mut state = nit_core::AgentsState::default();
    state
        .agents
        .push(test_lane("local", "Local", nit_core::AgentLaneKind::Mock));
    state.agents.push(test_lane(
        "claude",
        "Claude",
        nit_core::AgentLaneKind::Claude,
    ));
    state.claude_models = vec!["claude-sonnet-4-6".into(), "claude-opus-4-6".into()];

    sync_backend_model_lanes(&mut state, AgentsArg::All);

    assert_eq!(
        state.agents.len(),
        3,
        "expected local + 2 claude model lanes"
    );

    let mock_lane = state
        .agents
        .iter()
        .find(|lane| lane.id == "local")
        .expect("mock lane preserved");
    assert!(matches!(mock_lane.kind, nit_core::AgentLaneKind::Mock));

    assert!(
        !state.agents.iter().any(|lane| lane.id == "claude"),
        "placeholder claude lane should be expanded into per-model lanes"
    );
    for expected_model in ["claude-sonnet-4-6", "claude-opus-4-6"] {
        let expanded = state
            .agents
            .iter()
            .find(|lane| lane.id == expected_model)
            .unwrap_or_else(|| panic!("missing lane {expected_model}"));
        assert!(matches!(expanded.kind, nit_core::AgentLaneKind::Claude));
    }
}
