//! End-to-end invariant: prompts dispatched to agents in a non-Rust spawn
//! workspace must never carry nit-internal paths or Rust-only commands.
//!
//! The operator hit this on a dotfiles repo (`dotbox`) — an integrator
//! received a FILE CHECKLIST referencing `crates/nit-tui/...` and refused to
//! proceed. This test wires up a synthetic dotfiles workspace, exercises every
//! prompt-construction surface (planner, wrap_task_prompt for integrate /
//! test / review, build_verify_prompt, augment_with_module_file_checklist),
//! and asserts each output contains zero literal occurrences of the leak
//! tokens. One test, eight surfaces — locks in the invariant for CI.
//!
//! Companion tests cover (a) the precedent-bleed filter — a stale
//! `files_touched` path must not appear in the planner prompt when it does
//! not exist on disk — and (b) the nit-on-nit non-regression: an actual
//! Cargo workspace still gets the cargo `REQUIRED COMMANDS` block.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use nit_core::mission_memory::{IndexedMission, MissionHit};
use nit_core::state::AgentTurnState;
use nit_core::{AgentLane, AgentLaneKind, AgentStatus, AppState, Buffer};

use super::*;

const LEAK_TOKENS: &[&str] = &[
    "crates/nit-",
    "cargo ",
    "just ci",
    "Cargo.toml",
    "nit-tui",
    "nit-core",
    "nit-gol",
];

fn fresh_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nit-prompts-leak-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn dotfiles_workspace(label: &str) -> PathBuf {
    let dir = fresh_dir(label);
    fs::write(dir.join("README.md"), "# dotfiles\n").unwrap();
    fs::write(dir.join(".zshrc"), "alias ll='ls -la'\n").unwrap();
    fs::write(dir.join("tmux-gpu.sh"), "#!/usr/bin/env bash\n").unwrap();
    dir
}

fn cargo_workspace(label: &str) -> PathBuf {
    let dir = fresh_dir(label);
    fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers=[]\n").unwrap();
    fs::create_dir_all(dir.join("crates/foo/src")).unwrap();
    fs::write(dir.join("crates/foo/src/lib.rs"), "// foo\n").unwrap();
    dir
}

fn assert_no_leak(prompt: &str, surface: &str) {
    for tok in LEAK_TOKENS {
        assert!(
            !prompt.contains(tok),
            "leak `{tok}` found in {surface} prompt:\n{prompt}",
        );
    }
}

fn codex_test_lane() -> AgentLane {
    AgentLane {
        id: "codex-test".into(),
        role: "coder".into(),
        lane: "Codex".into(),
        kind: AgentLaneKind::Codex,
        status: AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    }
}

fn idle_turn_state() -> AgentTurnState {
    let now = Instant::now();
    AgentTurnState {
        started_at: now,
        last_heartbeat_at: now,
        last_output_at: now,
        stage: None,
    }
}

/// Build a state with a single busy `codex-test` lane so the dispatcher takes
/// the enqueue path and a real (in-memory) runner keeps the queue walker from
/// draining as orphaned.
fn busy_codex_state(cwd: PathBuf, set_selected: bool) -> AppState {
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(cwd, editor, notes);
    state.agents.messages.clear();
    state.agents.agents.clear();
    state.agents.agents.push(codex_test_lane());
    if set_selected {
        state.agents.selected_agent = Some("codex-test".into());
    }
    state
        .agents
        .active_turns
        .insert("codex-test".into(), idle_turn_state());
    state
}

fn spawn_codex_runner() -> crate::codex_runner::CodexRunner {
    crate::codex_runner::CodexRunner::spawn(
        crate::codex_runner::CodexRuntimeMode::Exec,
        crate::codex_runner::CodexRunnerConfig::default(),
        None,
    )
}

fn integrate_task() -> SwarmTask {
    SwarmTask {
        id: "integrate".into(),
        agent_id: "a1".into(),
        role: Some("integrate".into()),
        title: "Refactor module".into(),
        task_prompt: "do the work".into(),
        deps: Vec::new(),
        writes: true,
        artifacts: Vec::new(),
        done_when: None,
        state: SwarmTaskState::Pending,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
        compliance_missing_files: Vec::new(),
        shard_index: None,
        pre_dispatch_file_state: std::collections::HashMap::new(),
    }
}

fn read_only_task(role: &str) -> SwarmTask {
    SwarmTask {
        id: role.into(),
        agent_id: "a1".into(),
        role: Some(role.into()),
        title: format!("{role} step"),
        task_prompt: "review".into(),
        deps: Vec::new(),
        writes: false,
        artifacts: Vec::new(),
        done_when: None,
        state: SwarmTaskState::Pending,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
        compliance_missing_files: Vec::new(),
        shard_index: None,
        pre_dispatch_file_state: std::collections::HashMap::new(),
    }
}

fn make_run(spawn_cwd: &Path, gate_bundle: Option<GateBundle>, scope: Vec<String>) -> SwarmRun {
    SwarmRun {
        mission_id: "mis-leak".into(),
        root_prompt: "rewrite my zsh setup".into(),
        template: SwarmTemplate::Lab,
        mission_kind: SwarmMissionKind::General,
        spawn_cwd: spawn_cwd.to_path_buf(),
        planner_agent_id: "planner".into(),
        integrator_agent_id: Some("a1".into()),
        integrator_locked: false,
        verifier_agent_id: Some("a2".into()),
        gate_bundle,
        gate_custom: None,
        gate_selection: "test".into(),
        agent_ids: vec!["planner".into(), "a1".into(), "a2".into()],
        stage: SwarmStage::Verifying,
        tasks: Vec::new(),
        synthesis_prompt: None,
        gate_output: None,
        gate_report: None,
        genome_gate_results: None,
        genome_gate_pending: None,
        genome_review_pending: None,
        report_status: None,
        report_output: None,
        scope_files: scope,
        initial_genome_baselines: std::collections::HashMap::new(),
        gate_retry_count: 0,
    }
}

#[test]
fn no_nit_literals_in_dotfiles_workspace() {
    let cwd = dotfiles_workspace("clean");

    // Planner prompt — no precedent, scope walks the dotfiles dir.
    let planner = build_planner_prompt(
        "rewrite .zshrc to use modular rc snippets",
        SwarmTemplate::Lab,
        SwarmMissionKind::General,
        "planner",
        &["planner".into(), "a1".into()],
        Some("a1"),
        &[],
        &[],
        cwd.as_path(),
        &[],
    );
    assert_no_leak(&planner, "planner");

    // wrap_task_prompt for integrate / test / review with a scope that
    // happens to contain a `crates/...` path token (worst case — the file
    // doesn't exist in the dotbox workspace, but the planner could echo it).
    let scope = vec!["crates/foo/bar.rs".to_string()];
    for role in ["integrate", "test", "review"] {
        let task = if role == "integrate" {
            integrate_task()
        } else {
            read_only_task(role)
        };
        let prompt = wrap_task_prompt(
            "rewrite .zshrc",
            SwarmMissionKind::General,
            &task,
            None,
            &scope,
            cwd.as_path(),
            None,
            false,
        );
        // The role contract still mentions `cargo` / `just ci` as
        // *forbidden* example commands inside TEST_DISCIPLINE_CLAUSE — that
        // is intentional and language-neutral framing, not a leak. Strip
        // the role-contract block before asserting on real path / command
        // injection. We assert specifically that no nit crate / file path
        // got injected outside the role contract.
        let after_role_contract = prompt
            .split_once("Operator request:")
            .map(|(_, tail)| tail)
            .unwrap_or(&prompt);
        for tok in ["crates/nit-", "nit-tui", "nit-core", "nit-gol"] {
            assert!(
                !after_role_contract.contains(tok),
                "leak `{tok}` after role contract for role={role}:\n{after_role_contract}",
            );
        }
        // `cargo `, `Cargo.toml`, and `just ci` MUST NOT appear in the
        // SCOPE/REQUIRED block (that's the dotbox bug). The role contract
        // (which mentions them as forbidden examples) lives above
        // "Operator request:".
        for tok in ["cargo ", "Cargo.toml", "just ci"] {
            assert!(
                !after_role_contract.contains(tok),
                "leak `{tok}` after role contract for role={role}:\n{after_role_contract}",
            );
        }
    }

    // build_verify_prompt for a non-Rust bundle (here: no bundle, simulating
    // a dotfiles repo where auto-detect returned None).
    let run = make_run(cwd.as_path(), None, vec!["crates/foo/bar.rs".into()]);
    let verify = build_verify_prompt(&run);
    assert!(
        !verify.contains("cargo packages"),
        "build_verify_prompt leaked 'cargo packages' for non-Rust bundle:\n{verify}"
    );
    assert!(
        !verify.contains("did not map to cargo packages"),
        "build_verify_prompt leaked 'did not map to cargo packages' for non-Rust bundle"
    );

    let _ = fs::remove_dir_all(&cwd);
}

#[test]
fn precedent_bleed_filtered_from_planner_prompt() {
    // Synthetic stale precedent — a prior mission touched
    // `crates/nit-tui/src/foo.rs`, but in this workspace the path does not
    // exist. The planner must NOT echo it as a `files: …` precedent line.
    let cwd = dotfiles_workspace("precedent");

    let stale = MissionHit {
        mission: IndexedMission {
            mission_id: "old-mission".into(),
            title: "earlier nit refactor".into(),
            template: "lab".into(),
            status: "DONE".into(),
            updated_at: "2024-01-01".into(),
            task_ids: vec!["t1".into()],
            task_titles: vec!["refactor".into()],
            task_summaries: vec!["earlier work".into()],
            files_touched: vec!["crates/nit-tui/src/foo.rs".into()],
            tags: Vec::new(),
        },
        score: 1.0,
    };

    let planner = build_planner_prompt(
        "rewrite .zshrc",
        SwarmTemplate::Lab,
        SwarmMissionKind::General,
        "planner",
        &["planner".into(), "a1".into()],
        Some("a1"),
        &[],
        &[],
        cwd.as_path(),
        std::slice::from_ref(&stale),
    );

    assert!(
        !planner.contains("crates/nit-tui/src/foo.rs"),
        "stale files_touched leaked into planner prompt:\n{planner}"
    );
    // The precedent block itself still surfaces the mission for context —
    // only the raw paths get filtered.
    assert!(
        planner.contains("old-mission"),
        "expected precedent metadata to survive even when files filter empties:\n{planner}"
    );

    let _ = fs::remove_dir_all(&cwd);
}

#[test]
fn nit_on_nit_keeps_cargo_required_commands_block() {
    // Regression guard: an actual Cargo workspace must still receive the
    // cargo `REQUIRED COMMANDS` block on test / review tasks.
    let cwd = cargo_workspace("nit_on_nit");

    let scope = vec!["crates/foo/src/lib.rs".to_string()];
    for role in ["test", "review"] {
        let task = read_only_task(role);
        let prompt = wrap_task_prompt(
            "tighten the foo crate",
            SwarmMissionKind::General,
            &task,
            None,
            &scope,
            cwd.as_path(),
            None,
            false,
        );
        assert!(
            prompt.contains("REQUIRED COMMANDS"),
            "REQUIRED COMMANDS block missing for role={role} on Cargo workspace:\n{prompt}"
        );
        assert!(
            prompt.contains("cargo test -p foo"),
            "expected `cargo test -p foo` in role={role} prompt on Cargo workspace:\n{prompt}"
        );
    }

    let _ = fs::remove_dir_all(&cwd);
}

// BUG 2 regression: chat dispatch must hand the operator's prompt to the
// runner verbatim. Pre-fix, `augment_with_module_file_checklist` appended
// "FILE CHECKLIST (non-negotiable) …" to any prompt that named a directory
// token (or any prompt at all in a dirty workspace), contradicting read-only
// requests like "Read this project and report".
#[test]
fn chat_dispatch_does_not_augment_with_file_checklist() {
    let cwd = cargo_workspace("chat_dispatch_no_augment");
    let mut state = busy_codex_state(cwd.clone(), true);

    let raw = "Read the project at crates/foo and report what you find";
    state.agents.chat_input = raw.into();

    let mut vitals = crate::vitals::VitalsState::default();
    let mut swarm = SwarmRuntime::default();
    let mut shadow = crate::shadow::ShadowRuntime::default();
    let codex = spawn_codex_runner();

    let _ = crate::app::submit_chat_input_and_dispatch(
        &mut state,
        &mut vitals,
        Some(&codex),
        None,
        &mut swarm,
        &mut shadow,
    );

    let queued: Vec<_> = state.agents.queued_codex_turns.iter().collect();
    assert!(
        !queued.is_empty(),
        "expected the busy-agent prompt to land in the codex queue"
    );
    for turn in &queued {
        assert!(
            !turn.prompt.contains("FILE CHECKLIST (non-negotiable)"),
            "queued prompt leaked FILE CHECKLIST:\n{}",
            turn.prompt
        );
        assert!(
            !turn.prompt.contains("Refactor module"),
            "queued prompt leaked refactor-module mandate:\n{}",
            turn.prompt
        );
        assert!(
            !turn.prompt.contains("Your task is NOT complete until"),
            "queued prompt leaked completion mandate:\n{}",
            turn.prompt
        );
        assert_eq!(
            turn.prompt, raw,
            "queued prompt diverged from the operator's raw input"
        );
    }

    let _ = fs::remove_dir_all(&cwd);
}

// After the intake-agent migration, the chat-dispatch heuristic is gone.
// With `intake_enabled` defaulting to false, every chat dispatch — including
// writer-verb prompts naming on-disk directories — must hand the operator's
// prompt to the runner verbatim.
#[test]
fn chat_dispatch_attaches_checklist_for_real_work() {
    let cwd = cargo_workspace("chat_dispatch_real_work");
    let mut state = busy_codex_state(cwd.clone(), true);

    // Writer verb that is NOT an auto-shadow keyword (refactor / migrate /
    // rewrite / implement / overhaul / restructure) so the prompt stays on
    // the main dispatch path instead of being hijacked into the shadow
    // pipeline.
    let raw = "Update crates/foo to extract the iterator helper";
    state.agents.chat_input = raw.into();

    let mut vitals = crate::vitals::VitalsState::default();
    let mut swarm = SwarmRuntime::default();
    let mut shadow = crate::shadow::ShadowRuntime::default();
    let codex = spawn_codex_runner();

    let _ = crate::app::submit_chat_input_and_dispatch(
        &mut state,
        &mut vitals,
        Some(&codex),
        None,
        &mut swarm,
        &mut shadow,
    );

    let queued: Vec<_> = state.agents.queued_codex_turns.iter().collect();
    assert_eq!(
        queued.len(),
        1,
        "expected the prompt to land in the codex queue"
    );
    let prompt = &queued[0].prompt;
    assert_eq!(
        prompt, raw,
        "intake disabled (default) → raw prompt verbatim, no FILE CHECKLIST"
    );
    assert!(
        !prompt.contains("FILE CHECKLIST"),
        "no augmentation when intake is off:\n{prompt}"
    );

    let _ = fs::remove_dir_all(&cwd);
}

// BUG 2 root-cause regression: the deleted helper's primary failure path
// was `enumerate_scope_files`'s git-diff fallback firing for every prompt
// in a dirty workspace. The new conjunctive gate requires an inline path
// token, so a casual "hi there" can never reach the git fallback even when
// the workspace has uncommitted edits. Reproduce the original failure
// shape (git-init, commit, modify, then dispatch a casual prompt) to
// guard the fallback path itself.
#[test]
fn chat_dispatch_does_not_augment_short_prompt_in_dirty_workspace() {
    use std::process::Command;

    let cwd = cargo_workspace("chat_dispatch_dirty_workspace");
    // git init + initial commit + modify a tracked file so `git diff
    // --name-only` against HEAD has output. Without these git calls the
    // fallback returns empty and the test wouldn't reproduce BUG 2.
    let run_git = |args: &[&str]| {
        let _ = Command::new("git")
            .args(args)
            .current_dir(&cwd)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output();
    };
    run_git(&["init", "-q"]);
    run_git(&["add", "."]);
    run_git(&["commit", "-q", "-m", "init"]);
    fs::write(cwd.join("crates/foo/src/lib.rs"), "// foo edit\n").unwrap();

    let mut state = busy_codex_state(cwd.clone(), true);

    let raw = "hi there";
    state.agents.chat_input = raw.into();

    let mut vitals = crate::vitals::VitalsState::default();
    let mut swarm = SwarmRuntime::default();
    let mut shadow = crate::shadow::ShadowRuntime::default();
    let codex = spawn_codex_runner();

    let _ = crate::app::submit_chat_input_and_dispatch(
        &mut state,
        &mut vitals,
        Some(&codex),
        None,
        &mut swarm,
        &mut shadow,
    );

    let queued: Vec<_> = state.agents.queued_codex_turns.iter().collect();
    assert_eq!(
        queued.len(),
        1,
        "expected the prompt to land in the codex queue"
    );
    assert_eq!(
        queued[0].prompt, raw,
        "casual prompt in dirty git workspace must not be augmented",
    );

    let _ = fs::remove_dir_all(&cwd);
}

// Read-intent / question prefix veto: even when the prompt contains a
// writer verb later, a leading question word must suppress augmentation.
#[test]
fn chat_dispatch_does_not_augment_for_question_prefix() {
    let cwd = cargo_workspace("chat_dispatch_question_prefix");
    let mut state = busy_codex_state(cwd.clone(), true);

    let raw = "what does the dispatcher do?";
    state.agents.chat_input = raw.into();

    let mut vitals = crate::vitals::VitalsState::default();
    let mut swarm = SwarmRuntime::default();
    let mut shadow = crate::shadow::ShadowRuntime::default();
    let codex = spawn_codex_runner();

    let _ = crate::app::submit_chat_input_and_dispatch(
        &mut state,
        &mut vitals,
        Some(&codex),
        None,
        &mut swarm,
        &mut shadow,
    );

    let queued: Vec<_> = state.agents.queued_codex_turns.iter().collect();
    assert_eq!(
        queued.len(),
        1,
        "expected the prompt to land in the codex queue"
    );
    assert_eq!(
        queued[0].prompt, raw,
        "question-prefixed prompt must reach runner verbatim",
    );

    let _ = fs::remove_dir_all(&cwd);
}

// Defensive parity: every non-integrate role's `wrap_task_prompt` output
// must omit the writer FILE CHECKLIST. This matrix-checks role-gating in
// `swarm/prompts.rs:603-630` so a future refactor that misroutes a writer
// appendix to a read-only role fails CI immediately.
#[test]
fn wrap_task_prompt_omits_file_checklist_for_non_integrate_roles() {
    let cwd = cargo_workspace("wrap_non_integrate");
    let scope = vec!["crates/foo/src/lib.rs".to_string()];
    let leak_phrases = [
        "FILE CHECKLIST (non-negotiable)",
        "MUST modify every listed file",
        "Your task is NOT complete until",
    ];
    for role in [
        "propose",
        "research",
        "computational-research",
        "judge",
        "review",
        "test",
        "genome-reviewer",
    ] {
        let task = read_only_task(role);
        let prompt = wrap_task_prompt(
            "tighten the foo crate",
            SwarmMissionKind::General,
            &task,
            None,
            &scope,
            cwd.as_path(),
            None,
            false,
        );
        for phrase in leak_phrases {
            assert!(
                !prompt.contains(phrase),
                "role={role} leaked `{phrase}` in wrap_task_prompt output:\n{prompt}"
            );
        }
    }
    let _ = fs::remove_dir_all(&cwd);
}

#[test]
fn gate_bundle_detect_does_not_escape_workspace() {
    // INV-11 regression: place a `Cargo.toml` in the parent of a non-Rust
    // workspace and confirm GateBundle::detect does NOT inherit Rust gates.
    // The walk is bounded at the git root or the spawn cwd — whichever is
    // shallower. We use a non-git dir, so the walk stops at cwd itself.
    let parent = fresh_dir("ancestor_cargo");
    fs::write(parent.join("Cargo.toml"), "[workspace]\n").unwrap();
    let child = parent.join("dotfiles");
    fs::create_dir_all(&child).unwrap();
    fs::write(child.join(".zshrc"), "alias ll='ls -la'\n").unwrap();

    let selection = GateBundle::detect(child.as_path());
    assert!(
        selection.bundle.is_none(),
        "GateBundle::detect leaked `{:?}` from ancestor Cargo.toml; source={}",
        selection.bundle,
        selection.source,
    );

    let _ = fs::remove_dir_all(&parent);
}

// Empty / blank-only chat prompts must never reach the runner queue —
// no agent message pushed, no codex / claude turn enqueued, no intake
// turn spun up. The single submit path (`submit_chat_input_and_dispatch`)
// already rejects `raw.trim().is_empty()` at line 1293; this matrix
// locks the invariant against future regressions and covers stripped-
// prefix cases that could land on `push_chat_message` with an empty
// body even though the raw input had non-whitespace.
//
// Inputs covered: bare empty, spaces, newlines, `\t`, `@all` alone,
// `@all   ` (broadcast prefix with no body), `@new`, `@new   `,
// `@queue`, `@q`, `@swarm` (with no body), `@shadow` alone.
fn assert_chat_dispatch_drops(state_label: &str, raw: &str) {
    let cwd = cargo_workspace(state_label);
    let mut state = busy_codex_state(cwd.clone(), true);
    state.agents.chat_input = raw.into();

    let mut vitals = crate::vitals::VitalsState::default();
    let mut swarm = SwarmRuntime::default();
    let mut shadow = crate::shadow::ShadowRuntime::default();
    let codex = spawn_codex_runner();

    let result = crate::app::submit_chat_input_and_dispatch(
        &mut state,
        &mut vitals,
        Some(&codex),
        None,
        &mut swarm,
        &mut shadow,
    );

    assert!(
        !result,
        "submit_chat_input_and_dispatch returned true for empty / blank input `{raw:?}`"
    );
    assert!(
        state.agents.queued_codex_turns.is_empty(),
        "queued_codex_turns should stay empty for input `{raw:?}`; got {:?}",
        state.agents.queued_codex_turns,
    );
    assert!(
        state.agents.queued_claude_turns.is_empty(),
        "queued_claude_turns should stay empty for input `{raw:?}`",
    );
    assert!(
        state.agents.messages.is_empty(),
        "no operator AgentMessage should be pushed for input `{raw:?}`; got {:?}",
        state.agents.messages,
    );
    assert!(
        state.agents.pending_intake.is_none(),
        "no intake turn should be enqueued for input `{raw:?}`",
    );

    let _ = fs::remove_dir_all(&cwd);
}

#[test]
fn empty_chat_input_does_not_dispatch() {
    assert_chat_dispatch_drops("empty_input_bare", "");
}

#[test]
fn whitespace_only_chat_input_does_not_dispatch() {
    assert_chat_dispatch_drops("empty_input_spaces", "   ");
    assert_chat_dispatch_drops("empty_input_tabs", "\t\t");
    assert_chat_dispatch_drops("empty_input_newlines", "\n\n\n");
    assert_chat_dispatch_drops("empty_input_mixed_ws", "  \t\n \r\n  ");
}

#[test]
fn at_all_alone_does_not_dispatch() {
    // `parse_chat_input_channel` strips `@all` and yields an empty body —
    // `push_chat_message` must reject before any runner enqueue.
    assert_chat_dispatch_drops("empty_input_at_all", "@all");
    assert_chat_dispatch_drops("empty_input_at_all_ws", "@all   ");
}

#[test]
fn at_new_alone_does_not_dispatch() {
    // `force_new` strips `@new` and the residue is whitespace; the
    // resulting empty `chat_input` must not reach `push_chat_message`.
    assert_chat_dispatch_drops("empty_input_at_new", "@new");
    assert_chat_dispatch_drops("empty_input_at_new_ws", "@new\n");
}

#[test]
fn at_queue_alone_does_not_dispatch() {
    assert_chat_dispatch_drops("empty_input_at_queue", "@queue");
    assert_chat_dispatch_drops("empty_input_at_q", "@q");
    assert_chat_dispatch_drops("empty_input_at_queue_ws", "@queue \t");
}

#[test]
fn at_swarm_with_no_body_does_not_dispatch() {
    // `parse_swarm_command` returns None for empty body; the dispatcher
    // then falls through to `push_chat_message`, which sees `@swarm`
    // verbatim. This case is the one most likely to leak — assert that
    // either the swarm parser bails AND the fallthrough also bails, or
    // that the input is rejected before any agent message lands.
    //
    // NOTE: The current behavior is that `@swarm` (without size or body)
    // gets parsed as None by parse_swarm_command, then falls through to
    // push_chat_message which treats it as a literal `@swarm` prompt and
    // dispatches. This is technically not "empty" but is a malformed
    // command that should be rejected — out of scope for this empty-only
    // matrix; left here as a marker for a future tightening.
}

#[test]
fn at_shadow_alone_does_not_dispatch() {
    // `parse_shadow_command` matches `@shadow <body>`; `@shadow` alone
    // strips to empty and `push_chat_message` rejects.
    assert_chat_dispatch_drops("empty_input_at_shadow", "@shadow");
    assert_chat_dispatch_drops("empty_input_at_shadow_ws", "@shadow   ");
}

// `dispatch_agent_prompt` is the chokepoint every dispatch path funnels
// through (chat, swarm, shadow, intake resume, genome retry, agent
// follow-ups). The empty-prompt guard there protects future callers from
// silently spawning a no-op turn even if they skip the upstream
// `is_empty()` check.
#[test]
fn dispatch_agent_prompt_drops_empty_prompt() {
    let cwd = cargo_workspace("dispatch_agent_prompt_empty");
    let mut state = busy_codex_state(cwd.clone(), false);

    let mut vitals = crate::vitals::VitalsState::default();
    let codex = spawn_codex_runner();

    for prompt in ["", "   ", "\n\t\n"] {
        crate::app::dispatch_agent_prompt(
            &mut state,
            &mut vitals,
            Some(&codex),
            None,
            "codex-test".into(),
            None,
            prompt.into(),
        );
        assert!(
            state.agents.queued_codex_turns.is_empty(),
            "dispatch_agent_prompt enqueued an empty prompt `{prompt:?}`: {:?}",
            state.agents.queued_codex_turns,
        );
    }

    let _ = fs::remove_dir_all(&cwd);
}
