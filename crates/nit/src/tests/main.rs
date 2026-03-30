use crate::agents::{
    parse_claude_models_from_binary, parse_gemini_models_from_source, select_current_claude_models,
    select_current_gemini_models, sync_backend_model_lanes,
};
use crate::cli::AgentsArg;

#[test]
fn parses_gemini_models_from_backend_source() {
    let source = r#"
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
        parse_gemini_models_from_source(source),
        vec![
            "gemini-2.5-flash".to_string(),
            "gemini-2.5-pro".to_string(),
            "gemini-3-pro-preview".to_string(),
        ]
    );
}

#[test]
fn keeps_only_current_gemini_models_by_family() {
    let models = vec![
        "gemini-3-pro-preview".to_string(),
        "gemini-3.1-pro-preview-customtools".to_string(),
        "gemini-2.5-pro".to_string(),
        "gemini-3-flash-preview".to_string(),
        "gemini-2.5-flash".to_string(),
        "gemini-2.5-flash-lite".to_string(),
    ];

    assert_eq!(
        select_current_gemini_models(models),
        vec![
            "gemini-2.5-flash".to_string(),
            "gemini-2.5-flash-lite".to_string(),
            "gemini-2.5-pro".to_string(),
        ]
    );
}

#[test]
fn parses_claude_models_from_backend_binary_strings() {
    let binary = br#"
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

    assert_eq!(
        parse_claude_models_from_binary(binary),
        vec![
            "claude-3-7-sonnet".to_string(),
            "claude-haiku-4-5".to_string(),
            "claude-opus-4-5".to_string(),
            "claude-opus-4-6".to_string(),
            "claude-sonnet-4".to_string(),
            "claude-sonnet-4-6".to_string(),
        ]
    );
}

#[test]
fn keeps_only_current_claude_models_by_family() {
    let models = vec![
        "claude-3-5-haiku".to_string(),
        "claude-haiku-4-5".to_string(),
        "claude-3-7-sonnet".to_string(),
        "claude-sonnet-4".to_string(),
        "claude-sonnet-4-6".to_string(),
        "claude-opus-4-5".to_string(),
        "claude-opus-4-6".to_string(),
    ];

    assert_eq!(
        select_current_claude_models(models),
        vec![
            "claude-haiku-4-5".to_string(),
            "claude-opus-4-6".to_string(),
            "claude-sonnet-4-6".to_string(),
        ]
    );
}

#[test]
fn sync_backend_model_lanes_replaces_placeholder_backend_rows() {
    let mut agents = nit_core::AgentsState::default();
    agents.agents.push(nit_core::AgentLane {
        id: "local".into(),
        role: "Local".into(),
        lane: "Local".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    agents.agents.push(nit_core::AgentLane {
        id: "claude".into(),
        role: "Claude".into(),
        lane: "Claude".into(),
        kind: nit_core::AgentLaneKind::Claude,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    agents.claude_models = vec!["claude-sonnet-4-6".into(), "claude-opus-4-6".into()];

    sync_backend_model_lanes(&mut agents, AgentsArg::All);

    assert!(agents.agents.iter().any(|lane| lane.id == "local"));
    assert!(agents
        .agents
        .iter()
        .any(|lane| lane.id == "claude-sonnet-4-6"));
    assert!(agents
        .agents
        .iter()
        .any(|lane| lane.id == "claude-opus-4-6"));
    assert!(!agents.agents.iter().any(|lane| lane.id == "claude"));
}
