//! Genome-retry pipeline tests: agent retry budget, per-agent eval
//! batching, worker disk-eval flow, shadow main retry.

use super::*;

#[test]
fn shadow_main_agent_retry_fires_like_swarm_writer() {
    let mut state = state_for_test_in_workspace("shadow-main-retry");
    state.settings.genome.genome_context_enabled = true;

    let main_agent = "codex-main".to_string();

    // Seed a code file on disk. The retry path no longer applies any
    // size threshold (encoder auto-pass + parsimony detector cover
    // trivial-code protection), but the 200-line body keeps the test
    // realistic and matches the pre-fix fixture.
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

/// Regression for the latent attribution bug in the shadow-eval fallback at
/// `dispatch_turn_genome_evals`. When a runner fails to emit `FileWrite`
/// events (tool-format mismatch), the fallback recovers from the GLOBAL
/// `genome_shadow_evals` map. It must NOT claim paths another agent owns —
/// directly via `genome_turn_modified` or transitively via an
/// `ExclusiveWrite` substrate claim — otherwise this agent inherits another
/// writer's files and the next retry routes to the wrong agent.
#[test]
fn dispatch_turn_genome_evals_fallback_skips_paths_owned_by_other_agents() {
    let mut state = state_for_test_in_workspace("fallback-attribution");
    state.settings.genome.genome_context_enabled = true;

    let workspace = state.workspace_root.clone();
    let owned_by_a = workspace.join("a.rs");
    let claimed_by_b = workspace.join("b.rs");
    let unowned = workspace.join("u.rs");
    fs::write(&owned_by_a, "fn a() {}\n").unwrap();
    fs::write(&claimed_by_b, "fn b() {}\n").unwrap();
    fs::write(&unowned, "fn u() {}\n").unwrap();

    state
        .genome_turn_modified
        .insert("agent-A".into(), [owned_by_a.clone()].into_iter().collect());
    let claim_id = state.substrate.next_claim_id("agent-B");
    let claimed_at_gen = state.substrate.current_generation();
    state
        .substrate
        .assert_claim(nit_core::substrate::Claim {
            id: claim_id,
            kind: nit_core::substrate::ClaimKind::ExclusiveWrite,
            target: nit_core::substrate::ClaimTarget::File {
                path: claimed_by_b.clone(),
            },
            claimed_by: "agent-B".into(),
            claimed_at_gen,
            ttl_gens: 16,
            rationale: "seed".into(),
        })
        .expect("seed claim asserts cleanly");

    let seed_shadow = |tier| nit_core::GenomeShadowEval {
        tier,
        quality: "ok",
        consistency: 0.5,
        delta_label: "unchanged",
        is_new_file: false,
        at: Instant::now(),
    };
    state.genome_shadow_evals.insert(
        owned_by_a.clone(),
        seed_shadow(nit_core::GenomeTier::Spaceship),
    );
    state.genome_shadow_evals.insert(
        claimed_by_b.clone(),
        seed_shadow(nit_core::GenomeTier::Spaceship),
    );
    state.genome_shadow_evals.insert(
        unowned.clone(),
        seed_shadow(nit_core::GenomeTier::Spaceship),
    );

    let genome = crate::genome_worker::GenomeWorker::new();
    super::dispatch_turn_genome_evals(&mut state, &genome, "agent-C", &Some("mis-C".into()));

    let c_files = state
        .genome_turn_modified
        .get("agent-C")
        .cloned()
        .unwrap_or_default();
    assert!(
        c_files.contains(&unowned),
        "fallback should pick up the unowned file"
    );
    assert!(
        !c_files.contains(&owned_by_a),
        "fallback must not claim a path another agent's genome_turn_modified entry already owns"
    );
    assert!(
        !c_files.contains(&claimed_by_b),
        "fallback must not claim a path covered by another agent's ExclusiveWrite claim"
    );
}

/// Regression for the operator-reported bug: when two writers share a turn
/// and each has a degraded file, the retry must fan out per writer — one
/// retry prompt per owner, scoped to that owner's failed files only. The
/// dispatch must not collapse to a single retry, and it must not bleed one
/// agent's files into another's prompt.
///
/// Setup mirrors the `drain_genome_results` per-batch finalization path: A
/// owns [a1,a2,a3] with a2 degraded; B owns [b1,b2,b3] with b2 degraded.
/// After both batches finalize, two `build_genome_retry_prompt` calls fire
/// (one per agent), each scoped to its own failed file.
#[test]
fn multi_agent_genome_retry_dispatches_per_owner_scope() {
    let mut state = state_for_test_in_workspace("multi-agent-retry");
    state.settings.genome.genome_context_enabled = true;

    let mk_big = |label: &str| -> std::path::PathBuf {
        let path = state.workspace_root.join(format!("{label}.rs"));
        let body: String = (0..200)
            .map(|i| format!("fn f{i}() {{ let _ = {i}; }}\n"))
            .collect();
        fs::write(&path, body).unwrap();
        path
    };
    // Agent A's three files; a2 will be marked degraded.
    let a1 = mk_big("a1");
    let a2 = mk_big("a2");
    let a3 = mk_big("a3");
    // Agent B's three files; b2 will be marked degraded.
    let b1 = mk_big("b1");
    let b2 = mk_big("b2");
    let b3 = mk_big("b3");

    let agent_a = "agent-A".to_string();
    let agent_b = "agent-B".to_string();

    state.genome_turn_modified.insert(
        agent_a.clone(),
        [a1.clone(), a2.clone(), a3.clone()].into_iter().collect(),
    );
    state.genome_turn_modified.insert(
        agent_b.clone(),
        [b1.clone(), b2.clone(), b3.clone()].into_iter().collect(),
    );

    let mk_report = |path: &std::path::Path, tier: nit_core::GenomeTier| nit_core::GenomeReport {
        file_path: path.to_path_buf(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.6,
        tier,
        recommendations: Vec::new(),
        timestamp_ms: 1,
        grid_size: 32,
        parsimony: Default::default(),
        function_scores: Vec::new(),
    };
    // Spaceship baselines for all six files.
    for p in [&a1, &a2, &a3, &b1, &b2, &b3] {
        state
            .genome_baselines
            .insert((*p).clone(), mk_report(p, nit_core::GenomeTier::Spaceship));
    }
    // Post-turn reports: most files unchanged; a2 and b2 dropped a tier.
    for p in [&a1, &a3, &b1, &b3] {
        state
            .genome_reports
            .insert((*p).clone(), mk_report(p, nit_core::GenomeTier::Spaceship));
    }
    state
        .genome_reports
        .insert(a2.clone(), mk_report(&a2, nit_core::GenomeTier::Oscillator));
    state
        .genome_reports
        .insert(b2.clone(), mk_report(&b2, nit_core::GenomeTier::Oscillator));

    // Each batch finalises with a negative worst-delta (per-agent).
    state.genome_quality_deltas.insert(agent_a.clone(), -1);
    state.genome_quality_deltas.insert(agent_b.clone(), -1);

    // Build the retry prompts the way `drain_genome_results` does on each
    // batch finalisation — once per agent. Two distinct calls, not one.
    let (prompt_a, files_a) = super::build_genome_retry_prompt(&mut state, &agent_a)
        .expect("agent A's batch must produce its own retry");
    let (prompt_b, files_b) = super::build_genome_retry_prompt(&mut state, &agent_b)
        .expect("agent B's batch must produce its own retry");

    // Per-owner file scope: each retry list is exactly that agent's failed file.
    assert_eq!(
        files_a,
        vec![a2.clone()],
        "A's retry must scope to A's failed file only"
    );
    assert_eq!(
        files_b,
        vec![b2.clone()],
        "B's retry must scope to B's failed file only"
    );

    // Cross-bleed guard: no agent's prompt references another agent's files.
    let a2_name = a2.file_name().and_then(|n| n.to_str()).unwrap();
    let b2_name = b2.file_name().and_then(|n| n.to_str()).unwrap();
    assert!(
        prompt_a.contains(a2_name),
        "A's prompt must name its degraded file"
    );
    assert!(
        !prompt_a.contains(b2_name),
        "A's prompt must NOT mention B's degraded file"
    );
    assert!(
        prompt_b.contains(b2_name),
        "B's prompt must name its degraded file"
    );
    assert!(
        !prompt_b.contains(a2_name),
        "B's prompt must NOT mention A's degraded file"
    );

    // Each writer's retry budget advances independently — proves two
    // distinct dispatch decisions, not one combined retry. `GENOME_RETRY_LIMIT`
    // remains per-agent (mirrors the recon's confirmation).
    assert_eq!(
        state.genome_retry_counts.get(&agent_a).copied(),
        Some(1),
        "agent A's retry budget advanced once"
    );
    assert_eq!(
        state.genome_retry_counts.get(&agent_b).copied(),
        Some(1),
        "agent B's retry budget advanced once"
    );
    assert_eq!(
        state.genome_retry_counts.len(),
        2,
        "exactly two writers were retried — bug would have collapsed to one"
    );
}

/// Per-agent retry budgets stay isolated under the per-owner dispatch loop:
/// agent A exhausting its retry credits must not block agent B's first retry.
#[test]
fn multi_agent_genome_retry_budgets_are_independent_per_writer() {
    let mut state = state_for_test_in_workspace("multi-agent-retry-budget");
    state.settings.genome.genome_context_enabled = true;

    let mk_big = |label: &str| -> std::path::PathBuf {
        let path = state.workspace_root.join(format!("{label}.rs"));
        let body: String = (0..200)
            .map(|i| format!("fn f{i}() {{ let _ = {i}; }}\n"))
            .collect();
        fs::write(&path, body).unwrap();
        path
    };
    let file_a = mk_big("only_a");
    let file_b = mk_big("only_b");

    let agent_a = "agent-A".to_string();
    let agent_b = "agent-B".to_string();
    state
        .genome_turn_modified
        .insert(agent_a.clone(), [file_a.clone()].into_iter().collect());
    state
        .genome_turn_modified
        .insert(agent_b.clone(), [file_b.clone()].into_iter().collect());

    let baseline = |path: &std::path::Path| nit_core::GenomeReport {
        file_path: path.to_path_buf(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.6,
        tier: nit_core::GenomeTier::Spaceship,
        recommendations: Vec::new(),
        timestamp_ms: 1,
        grid_size: 32,
        parsimony: Default::default(),
        function_scores: Vec::new(),
    };
    let degraded = |path: &std::path::Path| nit_core::GenomeReport {
        file_path: path.to_path_buf(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.4,
        tier: nit_core::GenomeTier::Oscillator,
        recommendations: Vec::new(),
        timestamp_ms: 2,
        grid_size: 32,
        parsimony: Default::default(),
        function_scores: Vec::new(),
    };
    state
        .genome_baselines
        .insert(file_a.clone(), baseline(&file_a));
    state
        .genome_baselines
        .insert(file_b.clone(), baseline(&file_b));
    state
        .genome_reports
        .insert(file_a.clone(), degraded(&file_a));
    state
        .genome_reports
        .insert(file_b.clone(), degraded(&file_b));
    state.genome_quality_deltas.insert(agent_a.clone(), -1);
    state.genome_quality_deltas.insert(agent_b.clone(), -1);

    // Agent A is already at the per-agent retry ceiling.
    state
        .genome_retry_counts
        .insert(agent_a.clone(), super::GENOME_RETRY_LIMIT);

    // A's retry is blocked (budget exhausted) — must NOT silently consume B's.
    assert!(
        super::build_genome_retry_prompt(&mut state, &agent_a).is_none(),
        "A at the ceiling must not produce a retry"
    );
    // B's first retry still fires — its budget is independent.
    let (_prompt_b, files_b) = super::build_genome_retry_prompt(&mut state, &agent_b)
        .expect("B's first retry must fire even when A is exhausted");
    assert_eq!(files_b, vec![file_b]);
    assert_eq!(
        state.genome_retry_counts.get(&agent_b).copied(),
        Some(1),
        "B's budget advanced independently of A"
    );
    assert_eq!(
        state.genome_retry_counts.get(&agent_a).copied(),
        Some(super::GENOME_RETRY_LIMIT),
        "A's budget stayed at the ceiling — B's retry did not reset it"
    );
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

/// Operator-reported failure mode that drove v0.2.12: an agent
/// split a 341-line file into ~85-100-line submodules at Tier II,
/// some flagged for parsimony bloat. Pre-fix the retry path's
/// 120-line `GENOME_RETRY_MIN_LINES` floor silently skipped every
/// new submodule, so the retry never fired and the sub-tier code
/// landed. Post-fix the encoder's <20-sig-line auto-pass + the
/// parsimony detector are the only "is this substantive enough to
/// act on" gates; mission-authored sub-120-line files now fire
/// retries when they're below tier.
#[test]
fn small_new_file_below_tier_now_fires_retry() {
    let mut state = state_for_test_in_workspace("small-new-file-retry");
    state.settings.genome.genome_context_enabled = true;

    let agent = "claude-opus-4-7#swarm-mis-001-clone-01".to_string();

    // Build a 60-line Python file (well under the old 120 floor, well
    // over the encoder's ~20 sig-line auto-pass) — exactly the
    // submodule shape the screenshot reported.
    let file_path = state
        .workspace_root
        .join("core")
        .join("metatuner")
        .join("numerics.py");
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    let body: String = (0..60)
        .map(|i| format!("def fn_{i}():\n    return {i}\n"))
        .collect();
    fs::write(&file_path, body).unwrap();

    state
        .genome_turn_modified
        .insert(agent.clone(), [file_path.clone()].into_iter().collect());

    // No baseline — this is a new mission-authored file. Tier II
    // (Oscillator) is below the default min tier (Spaceship / III).
    let post_turn = nit_core::GenomeReport {
        file_path: file_path.clone(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.41,
        tier: nit_core::GenomeTier::Oscillator,
        recommendations: Vec::new(),
        timestamp_ms: 2,
        grid_size: 32,
        parsimony: Default::default(),
        function_scores: Vec::new(),
    };
    state.genome_reports.insert(file_path.clone(), post_turn);
    state.genome_quality_deltas.insert(agent.clone(), -1);

    let result = super::build_genome_retry_prompt(&mut state, &agent);
    let (_prompt, degraded) = result.expect(
        "60-line new file at Tier II below threshold MUST fire retry; \
         pre-v0.2.12 the 120-line floor silently skipped this",
    );
    assert_eq!(degraded, vec![file_path.clone()]);
    assert_eq!(state.genome_retry_counts.get(&agent).copied(), Some(1));
}

/// Sibling guard: a small new file flagged for parsimony bloat must
/// also fire retry now. Bloat is the writer's responsibility to
/// undo (consolidate over-split helpers, trim comment padding),
/// and the screenshot's `controller.py` was tier II with "2 hard-cap
/// breaches" — exactly this case. Pre-fix the line floor blocked
/// bloat-driven retries for small files; post-fix the parsimony
/// detector is authoritative regardless of size.
#[test]
fn small_new_file_with_bloat_now_fires_retry() {
    let mut state = state_for_test_in_workspace("small-bloated-new-file");
    state.settings.genome.genome_context_enabled = true;

    let agent = "claude-opus-4-7#swarm-mis-001-clone-02".to_string();

    let file_path = state
        .workspace_root
        .join("core")
        .join("metatuner")
        .join("controller.py");
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    let body: String = (0..80)
        .map(|i| format!("def fn_{i}():\n    return {i}\n"))
        .collect();
    fs::write(&file_path, body).unwrap();

    state
        .genome_turn_modified
        .insert(agent.clone(), [file_path.clone()].into_iter().collect());

    // Tier II AND parsimony bloat detected — the worst case the
    // screenshot reported. With v0.2.12 either signal alone is enough
    // to trigger retry, and the combination definitively must.
    let bloated_parsimony = nit_core::ParsimonyInfo {
        bloat_detected: true,
        ..Default::default()
    };
    let post_turn = nit_core::GenomeReport {
        file_path: file_path.clone(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.20,
        tier: nit_core::GenomeTier::Oscillator,
        recommendations: Vec::new(),
        timestamp_ms: 2,
        grid_size: 32,
        parsimony: bloated_parsimony,
        function_scores: Vec::new(),
    };
    state.genome_reports.insert(file_path.clone(), post_turn);
    // No quality_delta entry — the gate uses `any_bloat` to fall
    // through the `agent_delta >= 0 && !any_bloat` short-circuit, so
    // bloat alone (no baseline degradation) MUST be enough to fire.

    let result = super::build_genome_retry_prompt(&mut state, &agent);
    let (prompt, degraded) =
        result.expect("bloat-flagged new file MUST fire retry regardless of size");
    assert_eq!(degraded, vec![file_path.clone()]);
    assert!(
        prompt.contains("PARSIMONY BLOAT") || prompt.contains("parsimony bloat"),
        "retry prompt should call out the bloat finding; got:\n{prompt}"
    );
}
