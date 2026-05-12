//! Companion reference for the app/tests test directory. NOT loaded as a
//! Rust module — `app/mod.rs` declares the test mod via
//! `#[path = "tests.rs"] mod tests;` (resolving to `app/tests.rs`, the
//! flat parent file). This sibling file documents what each sub-file
//! covers so a future test reorganisation can read one place.

#![allow(dead_code)]

pub(super) const APP_TEST_SUBMODULE_COUNT: usize = 16;
pub(super) const TEMP_STATE_LABEL_PREFIX: &str = "nit-app";

pub(super) struct AppTestSubModule {
    pub file: &'static str,
    pub focus: &'static str,
}

pub(super) const APP_TEST_SUBMODULES: &[AppTestSubModule] = &[
    AppTestSubModule {
        file: "agent_chat.rs",
        focus: "chat input / paste / send / history / cursor / esc",
    },
    AppTestSubModule {
        file: "chat.rs",
        focus: "AgentsState.chat_input smoke",
    },
    AppTestSubModule {
        file: "codex.rs",
        focus: "AgentLaneKind variants + serde",
    },
    AppTestSubModule {
        file: "dispatch.rs",
        focus: "queue_len accounting + AgentStatus transitions",
    },
    AppTestSubModule {
        file: "editor.rs",
        focus: "editor / scratchpad / render-verify markdown",
    },
    AppTestSubModule {
        file: "genome_retry.rs",
        focus: "genome retry / dispatch eval batches / worker / shadow main",
    },
    AppTestSubModule {
        file: "helpers.rs",
        focus: "shared helper smoke",
    },
    AppTestSubModule {
        file: "keymap.rs",
        focus: "agent_ops / command_prompt / petri / file_tree / fuzzy / ctrl / parse_abort",
    },
    AppTestSubModule {
        file: "misc.rs",
        focus: "workspace_root + is_agent_busy + AgentLaneKind serde",
    },
    AppTestSubModule {
        file: "mission.rs",
        focus: "codex / claude / reset_context / archive / turn_completed",
    },
    AppTestSubModule {
        file: "missions.rs",
        focus: "AgentsState.missions / messages / active_turns shape",
    },
    AppTestSubModule {
        file: "mouse.rs",
        focus: "clicking / mouse_wheel / drag / agent_console mouse",
    },
    AppTestSubModule {
        file: "multipane.rs",
        focus: "multipane runtime drain",
    },
    AppTestSubModule {
        file: "popups.rs",
        focus: "fuzzy / help / artifacts / replay / strategy / global_archive / swarm_artifacts",
    },
    AppTestSubModule {
        file: "roster.rs",
        focus: "AgentLaneKind variants + AgentLane field shape",
    },
    AppTestSubModule {
        file: "swarm.rs",
        focus: "swarm / lab / parallel / propose_dispatch / shadow",
    },
];
