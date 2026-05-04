//! Shadow agents: hidden support agents (propose-a, propose-b, judge, review)
//! that augment a single selected agent with richer context before it answers
//! the user's prompt.
//!
//! Unlike `@swarm`, shadows do not plan a DAG of roles across the roster —
//! they run a small fixed pipeline behind the scenes for **one** agent:
//!
//! 1. Two proposer clones draft independent approaches in parallel.
//! 2. A judge clone compares the proposals and selects/synthesises the best.
//! 3. A review clone stress-tests the judged plan and surfaces risks.
//! 4. The main (user-selected) agent then runs with the review output prepended
//!    as additional context.
//!
//! Shadow lanes are created with `shadow: true` on `AgentLane`, which causes
//! the chat view and roster UI to hide them. The breather surfaces the current
//! stage ("Proposing...", "Judging...", "Reviewing...", "Finalizing...") so the
//! user has some feedback while the pipeline runs.

use std::collections::HashMap;

use nit_core::{AgentBusEvent, AgentStatus, AppState};

use crate::swarm::{
    copy_claude_runtime_metadata, copy_codex_runtime_metadata, insert_swarm_clone_lane,
};

pub const SHADOW_ROLES: &[&str] = &["propose-a", "propose-b", "judge", "review"];

/// How shadows are requested for a given prompt.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ShadowsMode {
    /// Explicitly disabled.
    Off,
    /// Explicitly enabled (user typed `@shadow ...`).
    On,
    /// Enable automatically for heavy prompts; otherwise off.
    #[default]
    Auto,
}

/// Parsed `@shadow <prompt>` command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShadowCommand {
    pub prompt: String,
}

/// Requires the prefix be followed by whitespace so `@shadows` or
/// `@shadowing` is not matched by accident.
pub fn parse_shadow_command(raw: &str) -> Option<ShadowCommand> {
    let trimmed = raw.trim_start();
    let rest = trimmed.strip_prefix("@shadow")?;
    // Must be end-of-string or followed by whitespace.
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let prompt = rest.trim().to_string();
    if prompt.is_empty() {
        return None;
    }
    Some(ShadowCommand { prompt })
}

/// Heuristic: is this prompt "heavy" enough to warrant shadow deliberation?
///
/// Triggers on long prompts (> 500 chars) OR a small, intentionally
/// conservative set of change-implying keywords. A question like "what does
/// this do?" should never spawn four extra agents.
pub fn should_auto_enable_shadows(prompt: &str) -> bool {
    const AUTO_SHADOW_MIN_CHARS: usize = 500;
    const KEYWORDS: &[&str] = &[
        "refactor",
        "migrate",
        "rewrite",
        "implement",
        "overhaul",
        "restructure",
    ];
    if prompt.chars().count() > AUTO_SHADOW_MIN_CHARS {
        return true;
    }
    let lower = prompt.to_ascii_lowercase();
    KEYWORDS.iter().any(|kw| lower.contains(kw))
}

/// A dispatch for one shadow agent (proposer, judge, reviewer, or main).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShadowDispatch {
    pub agent_id: String,
    pub prompt: String,
    pub mission_id: Option<String>,
    pub prompt_msg_idx: Option<usize>,
}

/// Stage of the shadow pipeline.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ShadowStage {
    Proposing,
    Judging,
    Reviewing,
    /// Reviewer done, but main is mid-turn — defer the shadow dispatch so it
    /// isn't queued behind unrelated work (which would misattribute responses
    /// and trigger premature cleanup).
    AwaitingMainIdle,
    Finalizing,
}

struct ShadowRun {
    main_agent_id: String,
    main_prompt: String,
    mission_id: Option<String>,
    prompt_msg_idx: Option<usize>,
    stage: ShadowStage,
    /// role → shadow lane id
    lanes: HashMap<String, String>,
    /// role → captured output
    outputs: HashMap<String, String>,
}

#[derive(Default)]
pub struct ShadowRuntime {
    /// Active runs keyed by main agent id (one concurrent run per main agent).
    runs: HashMap<String, ShadowRun>,
    next_run_id: u64,
}

impl ShadowRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// Is there an active shadow run for this main agent?
    pub fn has_run_for(&self, main_agent_id: &str) -> bool {
        self.runs.contains_key(main_agent_id)
    }

    /// True if `agent_id` is one of the shadow clones currently tracked.
    pub fn is_shadow_agent(&self, agent_id: &str) -> bool {
        self.runs
            .values()
            .any(|run| run.lanes.values().any(|id| id == agent_id))
    }

    /// Kick off a new shadow run for `main_agent_id`.
    ///
    /// Creates four hidden clones (propose-a, propose-b, judge, review) and
    /// returns the two proposer dispatches. The caller is responsible for
    /// augmenting each prompt with the genome landscape and dispatching —
    /// the workspace-scan runtime keeps `state.genome_reports` populated so
    /// proposers never need to wait for a per-dispatch prescan.
    pub fn start(
        &mut self,
        state: &mut AppState,
        main_agent_id: String,
        main_prompt: String,
        mission_id: Option<String>,
        prompt_msg_idx: Option<usize>,
    ) -> Option<Vec<ShadowDispatch>> {
        if self.runs.contains_key(&main_agent_id) {
            return None;
        }
        // Main agent must exist and be dispatchable.
        let main_exists = state
            .agents
            .agents
            .iter()
            .any(|lane| lane.id == main_agent_id && (lane.is_codex() || lane.is_claude()));
        if !main_exists {
            return None;
        }

        self.next_run_id = self.next_run_id.saturating_add(1);
        let run_id = format!("{:02}", self.next_run_id);

        let mut lanes: HashMap<String, String> = HashMap::new();
        for role in SHADOW_ROLES {
            let clone_id = ensure_shadow_lane(state, &main_agent_id, &run_id, role)?;
            lanes.insert((*role).to_string(), clone_id);
        }

        let proposer_dispatches = vec![
            ShadowDispatch {
                agent_id: lanes["propose-a"].clone(),
                prompt: build_propose_prompt("A", &main_prompt),
                mission_id: mission_id.clone(),
                prompt_msg_idx: None,
            },
            ShadowDispatch {
                agent_id: lanes["propose-b"].clone(),
                prompt: build_propose_prompt("B", &main_prompt),
                mission_id: mission_id.clone(),
                prompt_msg_idx: None,
            },
        ];

        let run = ShadowRun {
            main_agent_id: main_agent_id.clone(),
            main_prompt,
            mission_id,
            prompt_msg_idx,
            stage: ShadowStage::Proposing,
            lanes,
            outputs: HashMap::new(),
        };

        self.runs.insert(main_agent_id, run);
        Some(proposer_dispatches)
    }

    /// Process a `TurnCompleted` event. Returns dispatches to perform next.
    ///
    /// Events for agents not tracked by any run are ignored.
    pub fn handle_turn_completed(
        &mut self,
        state: &mut AppState,
        event_agent_id: &str,
        message: &str,
    ) -> Vec<ShadowDispatch> {
        // Find which run this agent belongs to (either shadow or main).
        let Some(main_agent_id) = self.run_id_for_agent(event_agent_id) else {
            return Vec::new();
        };
        let Some(run) = self.runs.get_mut(&main_agent_id) else {
            return Vec::new();
        };

        // Main agent finished.
        if event_agent_id == run.main_agent_id {
            match run.stage {
                ShadowStage::AwaitingMainIdle => {
                    // Prior turn just cleared — dispatch the deferred shadow prompt now.
                    return finalize_main_dispatch(run, state);
                }
                ShadowStage::Finalizing => {
                    // Shadow-augmented turn completed → tear down.
                    let lane_ids: Vec<String> = run.lanes.values().cloned().collect();
                    self.runs.remove(&main_agent_id);
                    cleanup_shadow_lanes(state, &lane_ids);
                }
                _ => {
                    // Unrelated prior-turn completion during Proposing/Judging/Reviewing.
                }
            }
            return Vec::new();
        }

        // Shadow finished: stash its output and maybe advance the stage.
        let role = role_of(run, event_agent_id);
        if let Some(role) = role {
            run.outputs.insert(role.clone(), message.to_string());
        }

        let output =
            |role: &str| -> &str { run.outputs.get(role).map(String::as_str).unwrap_or("") };
        match run.stage {
            ShadowStage::Proposing => {
                if run.outputs.contains_key("propose-a") && run.outputs.contains_key("propose-b") {
                    let prompt = build_judge_prompt(
                        &run.main_prompt,
                        output("propose-a"),
                        output("propose-b"),
                    );
                    let judge_id = run.lanes["judge"].clone();
                    let mission_id = run.mission_id.clone();
                    run.stage = ShadowStage::Judging;
                    return vec![ShadowDispatch {
                        agent_id: judge_id,
                        prompt,
                        mission_id,
                        prompt_msg_idx: None,
                    }];
                }
            }
            ShadowStage::Judging => {
                if run.outputs.contains_key("judge") {
                    let prompt = build_review_prompt(&run.main_prompt, output("judge"));
                    let review_id = run.lanes["review"].clone();
                    let mission_id = run.mission_id.clone();
                    run.stage = ShadowStage::Reviewing;
                    return vec![ShadowDispatch {
                        agent_id: review_id,
                        prompt,
                        mission_id,
                        prompt_msg_idx: None,
                    }];
                }
            }
            ShadowStage::Reviewing => {
                if run.outputs.contains_key("review") {
                    if crate::swarm::is_agent_busy(state, &run.main_agent_id) {
                        run.stage = ShadowStage::AwaitingMainIdle;
                        return Vec::new();
                    }
                    return finalize_main_dispatch(run, state);
                }
            }
            ShadowStage::AwaitingMainIdle | ShadowStage::Finalizing => {}
        }

        Vec::new()
    }

    /// On `TurnFailed` for a shadow agent, abort the run by flushing shadow
    /// lanes and falling back to dispatching the main prompt unaugmented.
    pub fn handle_turn_failed(
        &mut self,
        state: &mut AppState,
        event_agent_id: &str,
    ) -> Option<ShadowDispatch> {
        let main_agent_id = self.run_id_for_agent(event_agent_id)?;
        let run = self.runs.remove(&main_agent_id)?;
        let lane_ids: Vec<String> = run.lanes.values().cloned().collect();
        cleanup_shadow_lanes(state, &lane_ids);
        // If the failure was on the main agent itself, don't re-dispatch.
        if event_agent_id == run.main_agent_id {
            return None;
        }
        Some(ShadowDispatch {
            agent_id: run.main_agent_id,
            prompt: run.main_prompt,
            mission_id: run.mission_id,
            prompt_msg_idx: run.prompt_msg_idx,
        })
    }

    fn run_id_for_agent(&self, agent_id: &str) -> Option<String> {
        if self.runs.contains_key(agent_id) {
            return Some(agent_id.to_string());
        }
        self.runs
            .iter()
            .find(|(_, run)| run.lanes.values().any(|id| id == agent_id))
            .map(|(main_id, _)| main_id.clone())
    }

    /// Operator-driven cancellation of an in-flight shadow run for
    /// `main_agent_id`. Removes the run from the runtime (so any
    /// stragglers reaching `handle_turn_failed` become no-ops and don't
    /// re-dispatch the main prompt) and tears down the shadow lanes via
    /// `cleanup_shadow_lanes`. Returns the lane ids the run had spun
    /// up so the caller can route a `CancelTurn` to each one's runner —
    /// `cleanup_shadow_lanes` only purges the in-process bookkeeping;
    /// it cannot reach the live `codex`/`claude` child processes.
    pub fn abort_run(&mut self, state: &mut AppState, main_agent_id: &str) -> Vec<String> {
        let Some(run) = self.runs.remove(main_agent_id) else {
            return Vec::new();
        };
        let lane_ids: Vec<String> = run.lanes.values().cloned().collect();
        cleanup_shadow_lanes(state, &lane_ids);
        lane_ids
    }
}

fn role_of(run: &ShadowRun, agent_id: &str) -> Option<String> {
    run.lanes
        .iter()
        .find(|(_, id)| id.as_str() == agent_id)
        .map(|(role, _)| role.clone())
}

fn finalize_main_dispatch(run: &mut ShadowRun, state: &AppState) -> Vec<ShadowDispatch> {
    // Compute the landscape for the main agent the same way
    // `augment_shadow_prompt_with_landscape` does for the shadow lanes —
    // single-agent mode uses the active editor buffer as the scope. The
    // main agent's id isn't a shadow lane id, so the dispatch-time augment
    // helper skips it; injecting it here ensures the main agent has raw
    // metric data even when the shadows' digests don't cite numbers.
    let landscape = landscape_for_main(state);
    let prompt = build_final_prompt(run, landscape.as_deref());
    // FILE CHECKLIST gating moved to the intake agent (see
    // `crates/nit-tui/src/intake.rs`). The shadow pipeline's main writer
    // already receives the judge's binding plan — including any file
    // paths the judge identified — so a separate FILE CHECKLIST here
    // would duplicate the contract. Operators who want the explicit
    // checklist should run a non-shadow chat dispatch with
    // `intake_enabled = true`.
    let agent_id = run.main_agent_id.clone();
    let mission_id = run.mission_id.clone();
    let prompt_msg_idx = run.prompt_msg_idx;
    run.stage = ShadowStage::Finalizing;
    vec![ShadowDispatch {
        agent_id,
        prompt,
        mission_id,
        prompt_msg_idx,
    }]
}

fn landscape_for_main(state: &AppState) -> Option<String> {
    let path = state.editor_buffer().path()?;
    let rel = path
        .strip_prefix(&state.workspace_root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();
    let scope: Vec<String> = vec![rel];
    crate::app::build_propose_genome_landscape(state, &scope, Some("integrate"))
}

// ---------------------------------------------------------------------------
// Lane creation / cleanup
// ---------------------------------------------------------------------------

/// Build the canonical shadow lane id for a (base, run, role) tuple.
pub fn shadow_lane_id(base_id: &str, run_id: &str, role: &str) -> String {
    format!("{base_id}#shadow-{run_id}-{role}")
}

/// Parse a shadow lane id back into its components.
pub fn parse_shadow_lane_id(lane_id: &str) -> Option<(&str, &str, &str)> {
    let (base, rest) = lane_id.split_once("#shadow-")?;
    let (run_id, role) = rest.split_once('-')?;
    Some((base, run_id, role))
}

fn ensure_shadow_lane(
    state: &mut AppState,
    base_id: &str,
    run_id: &str,
    role: &str,
) -> Option<String> {
    let clone_id = shadow_lane_id(base_id, run_id, role);
    if state.agents.agents.iter().any(|lane| lane.id == clone_id) {
        return Some(clone_id);
    }

    let base_lane = state
        .agents
        .agents
        .iter()
        .find(|lane| lane.id == base_id)
        .cloned()?;

    let mut lane = base_lane.clone();
    lane.id = clone_id.clone();
    lane.role = format!("{} (shadow {role})", base_lane.role.trim());
    lane.status = AgentStatus::Idle;
    lane.heartbeat_age_secs = 0;
    lane.queue_len = 0;
    lane.current_mission = None;
    lane.last_message = String::new();
    lane.shadow = true;

    insert_swarm_clone_lane(state, base_id, lane);
    copy_codex_runtime_metadata(state, base_id, &clone_id);
    copy_claude_runtime_metadata(state, base_id, &clone_id);
    Some(clone_id)
}

fn cleanup_shadow_lanes(state: &mut AppState, lane_ids: &[String]) {
    for id in lane_ids {
        state.agents.active_turns.remove(id);
        // Thread/session bookkeeping — shadow clones open fresh contexts on
        // each dispatch, but the bus events still record an id we must drop.
        state.agents.codex_thread_ids.remove(id);
        state.agents.codex_used_tokens.remove(id);
        state.agents.codex_context_remaining_pct.remove(id);
        state.agents.codex_turn_prompt_idx.remove(id);
        state.agents.claude_session_ids.remove(id);
        state.agents.claude_used_tokens.remove(id);
        state.agents.claude_context_remaining_pct.remove(id);
        state.agents.claude_turn_prompt_idx.remove(id);
        for map in state.agents.codex_mission_thread_ids.values_mut() {
            map.remove(id);
        }
        for map in state.agents.codex_mission_used_tokens.values_mut() {
            map.remove(id);
        }
        for map in state
            .agents
            .codex_mission_context_remaining_pct
            .values_mut()
        {
            map.remove(id);
        }
        for map in state.agents.claude_mission_session_ids.values_mut() {
            map.remove(id);
        }
        for map in state.agents.claude_mission_used_tokens.values_mut() {
            map.remove(id);
        }
        for map in state
            .agents
            .claude_mission_context_remaining_pct
            .values_mut()
        {
            map.remove(id);
        }
        // Runtime/UI metadata copied from the base lane.
        state
            .agents
            .codex_effective_context_window_tokens
            .remove(id);
        state.agents.codex_default_reasoning_effort.remove(id);
        state.agents.codex_supported_reasoning_efforts.remove(id);
        state.agents.codex_selected_reasoning_effort.remove(id);
        state
            .agents
            .claude_effective_context_window_tokens
            .remove(id);
        state.agents.claude_default_effort.remove(id);
        state.agents.claude_supported_efforts.remove(id);
        state.agents.claude_selected_effort.remove(id);
        state.agents.swarm_role_by_agent_id.remove(id);
        state.agents.swarm_priority_agent_ids.remove(id);
        state.agents.roster_tree_collapsed_agent_ids.remove(id);
        state.genome_turn_modified.remove(id);
        // Orphan queued turns that never dispatched.
        state
            .agents
            .queued_codex_turns
            .retain(|turn| &turn.agent_id != id);
        state
            .agents
            .queued_claude_turns
            .retain(|turn| &turn.agent_id != id);
    }
    state
        .agents
        .agents
        .retain(|lane| !lane_ids.contains(&lane.id));
}

// ---------------------------------------------------------------------------
// Prompt builders
// ---------------------------------------------------------------------------

/// System-awareness preamble shared by every shadow role. Each shadow agent
/// runs in its own isolated context, so it won't see the full genome
/// instructions the main agent receives through the normal dispatch pipeline
/// unless we paste them inline. This ensures proposers, judges, and reviewers
/// all factor nit's quality/parsimony constraints into their outputs.
fn nit_system_awareness() -> String {
    let mut out = String::from(
        "## NIT SYSTEM AWARENESS\n\
         You are operating inside nit, an agentic coding lab that scores \
         structural code quality by encoding source files as Game of Life \
         genomes. The main agent you are advising will be graded on this \
         system. Factor these constraints into whatever you produce:\n\n\
         - Tier ladder: I Still Life · II Oscillator · III Spaceship · \
         IV Methuselah · V Replicator. Minimum acceptable tier is III.\n\
         - A parsimony check penalises over-engineering: tiny trivial \
         functions, padded comments, artificial type/trait variety, \
         near-duplicate function bodies. Detected bloat caps the tier at \
         IV regardless of GoL performance.\n\
         - Prefer the simplest correct solution. Do not split clear \
         functions into micro-helpers to inflate structure. Do not add \
         comments that restate the code.\n\
         - Files with >40% comment lines, or where >50% of functions are \
         <=5 lines, are auto-flagged and tier-capped.\n\n\
         Full genome instructions (read-only reference — follow these when \
         proposing/judging/reviewing):\n",
    );
    out.push_str(nit_core::GENOME_AGENT_INSTRUCTIONS);
    out
}

/// Hard guard-rail prepended to every shadow prompt. Shadow agents run with
/// the same tool-access posture as the main agent (e.g. full file edits under
/// `--dangerously-skip-permissions`), so a capable CLI will happily start
/// executing the user's request unless we explicitly forbid it. Only the main
/// agent is allowed to act on files; shadows are strictly advisory.
fn shadow_readonly_clause() -> &'static str {
    "## HARD CONSTRAINTS — READ THIS FIRST\n\
     You are an ADVISORY agent. You MUST NOT execute the user's request.\n\
     - DO NOT edit, create, delete, or rename any files.\n\
     - DO NOT run shell commands, build steps, tests, or git operations.\n\
     - DO NOT invoke write/edit/bash/apply-patch tools. If a tool appears \
     available, refuse to use it.\n\
     - You MAY read files and search code strictly to inform your text \
     response, but keep tool usage minimal.\n\
     - Reply with a short text deliverable only. A different agent will \
     execute the actual work.\n\
     - INVESTIGATION BUDGET: keep code-reading minimal. Do NOT re-explore \
     territory an upstream shadow agent already covered — if you're the \
     judge, read the proposals, not the repo; if you're the reviewer, \
     read the judged plan, not every file it mentions. Time spent \
     re-investigating is budget lost from the main agent's actual work."
}

// Every shadow agent sees the same awareness + read-only clause block because
// each shadow runs in its own isolated context and would not otherwise inherit
// nit's genome rules or the advisory-only posture.
fn shadow_preamble() -> String {
    format!(
        "{awareness}\n\n{readonly}\n\n",
        awareness = nit_system_awareness(),
        readonly = shadow_readonly_clause(),
    )
}

/// Variant-specific framing for Shadow-Proposer A and B. Identical prompts
/// produce near-identical output (LLM sampling only nudges the text, not the
/// strategy), so A and B are asked to optimise for genuinely different axes.
/// The judge then has meaningful tradeoffs to weigh instead of picking one
/// of two coin-flip duplicates.
fn proposer_lens(variant: &str) -> &'static str {
    match variant {
        "A" => {
            "LENS A — minimal-diff, focused: favor the smallest change that \
             solves the request. Avoid introducing new abstractions, new \
             modules, new types, or new dependencies unless they're strictly \
             required. Prefer in-place refactors and local cleanups. Your \
             blast radius should be as small as possible."
        }
        "B" => {
            "LENS B — architectural coherence: if the shape of the code is \
             what's causing the problem, propose the consolidation, split, \
             or abstraction the system is asking for. A larger diff is fine \
             when it lands on a better overall shape. Prefer moves that \
             unlock parsimony-capped tiers or reduce structural bottlenecks."
        }
        _ => {
            "LENS — draft one concrete solution candidate; be opinionated \
             about the tradeoffs you took."
        }
    }
}

fn render_role_contract(role: &str) -> String {
    // Pull the same role contract the swarm parallel template uses. Shadow
    // proposers/judges/reviewers need the SAME genome-awareness, coverage,
    // and landscape-grounding constraints as their swarm counterparts — any
    // divergence here means single-agent mode produces weaker proposals
    // than parallel mode for the same user request.
    let mut out = String::from("## ROLE CONTRACT\n");
    out.push_str("- Act strictly as the assigned role for this task.\n");
    for line in crate::swarm::role_contract_lines(role) {
        out.push_str(&format!("- {line}\n"));
    }
    out
}

fn build_propose_prompt(variant: &str, user_prompt: &str) -> String {
    format!(
        "{preamble}## YOUR ROLE\n\
         You are Shadow-Proposer-{variant}, a hidden support agent drafting \
         ONE concrete solution candidate for the following user request. \
         Work independently; do NOT coordinate with the other proposer. A \
         judge will compare your proposal against the other variant after \
         both land, then a reviewer stress-tests the winner. The main agent \
         executes the final plan.\n\n\
         {lens}\n\n\
         {role_contract}\n\n\
         ## USER REQUEST\n{user_prompt}\n",
        preamble = shadow_preamble(),
        lens = proposer_lens(variant),
        role_contract = render_role_contract("propose"),
    )
}

fn build_judge_prompt(user_prompt: &str, proposal_a: &str, proposal_b: &str) -> String {
    format!(
        "{preamble}## YOUR ROLE\n\
         You are Shadow-Judge, a hidden support agent. Two proposers drafted \
         independent approaches under DIFFERENT framings: Proposal A optimises \
         for minimal diff, Proposal B optimises for architectural coherence. \
         Compare them on the axes below and produce a SINGLE binding plan \
         for the main agent to execute.\n\n\
         DECISION AXES:\n\
         - Correctness: does the approach actually solve the user's request?\n\
         - Landscape fit: does it target the lowest-tier / highest-leverage \
           files surfaced in the GENOME LANDSCAPE?\n\
         - Parsimony: does it avoid gaming metrics through over-engineering?\n\
         - Blast radius: is the diff scoped to what the user asked for?\n\n\
         Do NOT silently pick Proposal A (position bias is the most common \
         judge failure). If the proposals agree, identify risks neither \
         addressed. If they disagree, name the disagreement, name the axis, \
         and rule on it with a cited reason.\n\n\
         {role_contract}\n\n\
         ## USER REQUEST\n{user_prompt}\n\n\
         ## PROPOSALS TO EVALUATE (2 — read ALL carefully before ruling)\n\n\
         ### Proposal A (minimal-diff lens)\n{proposal_a}\n\n\
         ### Proposal B (architectural-coherence lens)\n{proposal_b}\n",
        preamble = shadow_preamble(),
        role_contract = render_role_contract("judge"),
    )
}

fn build_review_prompt(user_prompt: &str, judged_plan: &str) -> String {
    format!(
        "{preamble}## YOUR ROLE\n\
         You are Shadow-Reviewer, a hidden support agent. Stress-test the \
         judged plan below: look for missed edge cases, broken assumptions, \
         unstated dependencies, and concrete file paths the plan should \
         touch but doesn't. Flag anything that risks tripping nit's \
         parsimony detector or dropping the tier below III.\n\n\
         {role_contract}\n\n\
         ## USER REQUEST\n{user_prompt}\n\n\
         ## JUDGED PLAN (stress-test this)\n{judged_plan}\n",
        preamble = shadow_preamble(),
        role_contract = render_role_contract("review"),
    )
}

fn build_final_prompt(run: &ShadowRun, landscape_section: Option<&str>) -> String {
    let empty = String::new();
    let propose_a = run.outputs.get("propose-a").unwrap_or(&empty);
    let propose_b = run.outputs.get("propose-b").unwrap_or(&empty);
    let judge = run.outputs.get("judge").unwrap_or(&empty);
    let review = run.outputs.get("review").unwrap_or(&empty);
    let landscape_block = landscape_section
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("{s}\n"))
        .unwrap_or_default();
    format!(
        "## IMPLEMENTATION PLAN (BINDING — follow verbatim unless technically impossible)\n\
         Four hidden support agents (propose-a, propose-b, judge, review) \
         analysed your request before this turn. The judge's plan below is \
         the authoritative plan for this task. Treat specific file paths, \
         identifiers, constants, and ordering as fixed requirements, not \
         suggestions.\n\n\
         You MAY deviate ONLY when (a) the judge's recommendation directly \
         contradicts the user's request, or (b) it is genuinely technically \
         impossible (non-existent type, broken compile invariant). \
         \"It might break tests\", \"it's risky\", \"it's too ambitious\", \
         \"I'll do a safer subset\" are NOT valid deviations.\n\n\
         If you do override, briefly state *which* recommendation you \
         overrode and *why* in your reply so the user can evaluate the call. \
         Silent override defeats the point of having the shadows.\n\n\
         ### Judge's recommended plan (BINDING)\n{judge}\n\n\
         ### Reviewer's risks and corrections (MUST address or rebut)\n{review}\n\n\
         ### Proposal A — minimal-diff lens (input, for reference)\n{propose_a}\n\n\
         ### Proposal B — architectural-coherence lens (input, for reference)\n{propose_b}\n\n\
         {landscape_block}## USER REQUEST\n{user_prompt}\n",
        user_prompt = run.main_prompt,
    )
}

/// Inspect live state and derive a stage label for shadow activity.
///
/// When `main_agent_id` is `Some(id)`, only shadow lanes whose base resolves
/// to that id are considered — this is what the breather uses so a shadow
/// run on agent A doesn't leak its stage into agent B's view.
///
/// When `main_agent_id` is `None`, any shadow lane counts (legacy "is
/// anything shadowing?" query).
///
/// Returns the highest-priority active stage:
///   * any proposer active → "Proposing"
///   * judge active → "Judging"
///   * reviewer active → "Reviewing"
///   * none active, but matching shadow lanes exist → "Finalizing"
///
/// Returns `None` if there are no matching shadow lanes. This avoids
/// threading `&ShadowRuntime` into widget code.
pub fn shadow_stage_label_from_state(
    state: &AppState,
    main_agent_id: Option<&str>,
) -> Option<&'static str> {
    let shadow_lanes: Vec<&str> = state
        .agents
        .agents
        .iter()
        .filter(|lane| lane.shadow)
        .filter(|lane| match main_agent_id {
            Some(id) => parse_shadow_lane_id(&lane.id)
                .map(|(base, _, _)| base == id)
                .unwrap_or(false),
            None => true,
        })
        .map(|lane| lane.id.as_str())
        .collect();
    if shadow_lanes.is_empty() {
        return None;
    }

    let mut propose_active = false;
    let mut judge_active = false;
    let mut review_active = false;
    for id in shadow_lanes.iter() {
        if !state.agents.active_turns.contains_key(*id) {
            continue;
        }
        if let Some((_base, _run, role)) = parse_shadow_lane_id(id) {
            match role {
                "propose-a" | "propose-b" => propose_active = true,
                "judge" => judge_active = true,
                "review" => review_active = true,
                _ => {}
            }
        }
    }

    if propose_active {
        Some("Proposing")
    } else if judge_active {
        Some("Judging")
    } else if review_active {
        Some("Reviewing")
    } else {
        Some("Finalizing")
    }
}

// ---------------------------------------------------------------------------
// Event-bus adapter
// ---------------------------------------------------------------------------

/// Outcome of applying a bus event to the shadow runtime — ready to be fed
/// back into the caller's `dispatch_agent_prompt` helper.
#[derive(Default, Debug)]
pub struct ShadowEventOutcome {
    pub dispatches: Vec<ShadowDispatch>,
}

impl ShadowRuntime {
    /// Process an agent bus event, returning any follow-up dispatches.
    pub fn handle_event_outcome(
        &mut self,
        state: &mut AppState,
        event: &AgentBusEvent,
    ) -> ShadowEventOutcome {
        let mut outcome = ShadowEventOutcome::default();
        match event {
            AgentBusEvent::TurnCompleted {
                agent_id, message, ..
            } => {
                outcome.dispatches = self.handle_turn_completed(state, agent_id, message);
            }
            AgentBusEvent::TurnFailed { agent_id, .. } => {
                if let Some(d) = self.handle_turn_failed(state, agent_id) {
                    outcome.dispatches.push(d);
                }
            }
            _ => {}
        }
        outcome
    }
}

#[cfg(test)]
#[path = "tests/shadow.rs"]
mod tests;
