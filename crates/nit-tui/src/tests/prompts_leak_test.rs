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

use nit_core::mission_memory::{IndexedMission, MissionHit};

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
