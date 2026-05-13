//! Genome-retry pipeline tests: agent retry budget, per-agent eval
//! batching, worker disk-eval flow, shadow main retry.

use super::*;

#[test]
fn shadow_main_agent_retry_fires_like_swarm_writer() {
    let mut state = state_for_test_in_workspace("shadow-main-retry");
    state.settings.genome.genome_context_enabled = true;

    let main_agent = "codex-main".to_string();

    // Seed a >=120-line code file (retry threshold) on disk. Synthetic body
    // with enough lines to pass GENOME_RETRY_MIN_LINES.
    let file_path = state.workspace_root.join("src").join("big.rs");
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    let body: String = (0..200)
        .map(|i| format!("fn f{i}() {{ let _ = {i}; }}\n"))
        .collect();
    fs::write(&file_path, body).unwrap();

    // Simulate the post-turn state: the main agent modified this file
    // during its shadow-augmented turn.
    state.genome_turn_modified.insert(
        main_agent.clone(),
        [file_path.clone()].into_iter().collect(),
    );

    // Baseline at Spaceship (III), post-turn at Oscillator (II) — tier drop.
    let baseline = nit_core::GenomeReport {
        file_path: file_path.clone(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.6,
        tier: nit_core::GenomeTier::Spaceship,
        recommendations: Vec::new(),
        timestamp_ms: 1,
        grid_size: 32,
        parsimony: Default::default(),
        function_scores: Vec::new(),
    };
    let post_turn = nit_core::GenomeReport {
        file_path: file_path.clone(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.4,
        tier: nit_core::GenomeTier::Oscillator,
        recommendations: Vec::new(),
        timestamp_ms: 2,
        grid_size: 32,
        parsimony: Default::default(),
        function_scores: Vec::new(),
    };
    state.genome_baselines.insert(file_path.clone(), baseline);
    state.genome_reports.insert(file_path.clone(), post_turn);

    // This is what `drain_genome_results` would have written after the
    // authoritative eval batch finalised: delta = -1 (degraded).
    state.genome_quality_deltas.insert(main_agent.clone(), -1);

    // Build the retry prompt exactly the way the swarm-parallel integrator
    // retry path does.
    let result = super::build_genome_retry_prompt(&mut state, &main_agent);
    let (prompt, degraded) = result.expect("shadow main retry should fire for a tier drop");

    assert_eq!(degraded.len(), 1);
    assert_eq!(degraded[0], file_path);
    assert!(prompt.contains("GENOME QUALITY DEGRADED"));
    assert!(prompt.contains("automatic retry 1/3"));
    let needle = std::path::PathBuf::from("src").join("big.rs");
    assert!(
        prompt.contains(&*needle.to_string_lossy()),
        "prompt should reference the degraded file path; got:\n{prompt}"
    );

    // Budget advances per-agent, same as swarm.
    assert_eq!(
        state.genome_retry_counts.get(&main_agent).copied(),
        Some(1),
        "per-agent retry count increments on main-agent retry"
    );
}

// Complement: once the main agent's retry budget is exhausted, the
// mechanism stops firing — matches swarm parallel behaviour where each
// writer has its own budget capped at GENOME_RETRY_LIMIT.
#[test]
fn shadow_main_agent_retry_respects_per_agent_budget() {
    let mut state = state_for_test_in_workspace("shadow-main-retry-budget");
    state.settings.genome.genome_context_enabled = true;

    let main_agent = "codex-main".to_string();
    let file_path = state.workspace_root.join("src").join("big.rs");
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    let body: String = (0..200)
        .map(|i| format!("fn f{i}() {{ let _ = {i}; }}\n"))
        .collect();
    fs::write(&file_path, body).unwrap();

    state.genome_turn_modified.insert(
        main_agent.clone(),
        [file_path.clone()].into_iter().collect(),
    );
    let baseline = nit_core::GenomeReport {
        file_path: file_path.clone(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.6,
        tier: nit_core::GenomeTier::Spaceship,
        recommendations: Vec::new(),
        timestamp_ms: 1,
        grid_size: 32,
        parsimony: Default::default(),
        function_scores: Vec::new(),
    };
    let post_turn = nit_core::GenomeReport {
        file_path: file_path.clone(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.4,
        tier: nit_core::GenomeTier::Oscillator,
        recommendations: Vec::new(),
        timestamp_ms: 2,
        grid_size: 32,
        parsimony: Default::default(),
        function_scores: Vec::new(),
    };
    state.genome_baselines.insert(file_path.clone(), baseline);
    state.genome_reports.insert(file_path.clone(), post_turn);
    state.genome_quality_deltas.insert(main_agent.clone(), -1);

    // Already at the retry limit — no more firings.
    state
        .genome_retry_counts
        .insert(main_agent.clone(), super::GENOME_RETRY_LIMIT);

    assert!(
        super::build_genome_retry_prompt(&mut state, &main_agent).is_none(),
        "retry at the budget ceiling must not fire"
    );
}

// nit-tui's GENOME_RETRY_LIMIT and nit-core's ARBITER_RETRY_LIMIT both
// cap the per-agent retry budget. They are duplicated because nit-core
// can't depend on nit-tui, but the runtime invariants only hold when
// they agree: arbiter interventions must not outlive an agent's genome
// retries, and vice versa. This test pins the equality so a drift
// triggers a CI failure, not a silent behaviour change.
#[test]
fn genome_retry_limit_matches_arbiter_retry_limit() {
    assert_eq!(
        super::GENOME_RETRY_LIMIT,
        nit_core::ARBITER_RETRY_LIMIT,
        "GENOME_RETRY_LIMIT (app/genome_retry.rs) and ARBITER_RETRY_LIMIT \
         (nit-core/arbiters/mod.rs) must stay in sync"
    );
}

/// Regression: overlapping `TurnCompleted` events from parallel swarm agents
/// must not clobber each other's genome-eval state. Before per-agent batches,
/// a second `dispatch_turn_genome_evals` overwrote the single-slot pending
/// counter and agent_id, silently dropping the first agent's retry.
#[test]
fn dispatch_turn_genome_evals_tracks_per_agent_batches_independently() {
    let mut state = state_for_test_in_workspace("genome-batches");
    state.settings.genome.genome_context_enabled = true;

    let workspace = state.workspace_root.clone();
    let file_a1 = workspace.join("a1.rs");
    let file_a2 = workspace.join("a2.rs");
    let file_b1 = workspace.join("b1.rs");
    fs::write(&file_a1, "fn a1() {}\n").unwrap();
    fs::write(&file_a2, "fn a2() {}\n").unwrap();
    fs::write(&file_b1, "fn b1() {}\n").unwrap();

    state.genome_turn_modified.insert(
        "agent-A".into(),
        [file_a1.clone(), file_a2.clone()].into_iter().collect(),
    );
    state
        .genome_turn_modified
        .insert("agent-B".into(), [file_b1.clone()].into_iter().collect());

    let genome = crate::genome_worker::GenomeWorker::new();

    // Interleave the dispatches the way parallel swarm would.
    super::dispatch_turn_genome_evals(&mut state, &genome, "agent-A", &Some("mis-A".into()));
    super::dispatch_turn_genome_evals(&mut state, &genome, "agent-B", &Some("mis-B".into()));

    let batch_a = state
        .genome_eval_batches
        .get("agent-A")
        .expect("agent-A batch present");
    let batch_b = state
        .genome_eval_batches
        .get("agent-B")
        .expect("agent-B batch present");
    assert_eq!(batch_a.pending, 2, "A has 2 pending files");
    assert_eq!(batch_b.pending, 1, "B has 1 pending file");
    assert_eq!(batch_a.mission_id.as_deref(), Some("mis-A"));
    assert_eq!(batch_b.mission_id.as_deref(), Some("mis-B"));
}

/// Each authoritative eval request must carry the dispatching agent_id all
/// the way to the worker's output so `drain_genome_results` can route
/// decrements to the right batch.
#[test]
fn genome_worker_evaluate_from_disk_tags_result_with_agent_id() {
    let workspace = std::env::temp_dir().join(format!(
        "nit-genome-tag-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos(),
    ));
    fs::create_dir_all(&workspace).expect("create workspace");
    let path = workspace.join("tagged.rs");
    fs::write(&path, "fn main() {}\n").unwrap();

    let genome = crate::genome_worker::GenomeWorker::new();
    assert!(genome.evaluate_from_disk(path.clone(), "agent-X".into()));

    let result = genome
        .rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("eval result");
    assert_eq!(result.agent_id.as_deref(), Some("agent-X"));
    assert_eq!(result.path, path);
    assert!(!result.shadow);
    assert!(!result.save_eval);
    assert!(result.report.is_some());

    let _ = fs::remove_dir_all(&workspace);
}
