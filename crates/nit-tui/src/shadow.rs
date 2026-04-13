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

/// The four hidden roles the shadow pipeline always spawns.
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

/// Recognise an explicit `@shadow <prompt>` prefix.
///
/// Accepts leading whitespace and requires the prefix be followed by whitespace
/// (so `@shadows` or `@shadowing` is not matched by accident).
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

/// Heuristic: a prompt is "heavy" enough to benefit from shadow deliberation.
///
/// We trigger auto-shadows when:
///   * the prompt is long (> 500 chars), or
///   * it contains any of a small set of change-implying keywords.
///
/// The keyword list is intentionally conservative — we don't want questions
/// like "what does this do?" to spawn four extra agents.
pub fn should_auto_enable_shadows(prompt: &str) -> bool {
    if prompt.chars().count() > 500 {
        return true;
    }
    let lower = prompt.to_ascii_lowercase();
    const KEYWORDS: &[&str] = &[
        "refactor",
        "migrate",
        "rewrite",
        "implement",
        "overhaul",
        "restructure",
    ];
    KEYWORDS.iter().any(|kw| lower.contains(kw))
}

/// A single dispatch the runtime wants the caller to perform.
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
    Finalizing,
}

impl ShadowStage {
    fn label(self) -> &'static str {
        match self {
            ShadowStage::Proposing => "Proposing",
            ShadowStage::Judging => "Judging",
            ShadowStage::Reviewing => "Reviewing",
            ShadowStage::Finalizing => "Finalizing",
        }
    }
}

struct ShadowRun {
    #[allow(dead_code)] // kept for debug prints / future logging
    run_id: String,
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

    /// Return the user-visible stage hint for a main agent, if any.
    pub fn stage_hint_for_agent(&self, main_agent_id: &str) -> Option<&'static str> {
        self.runs.get(main_agent_id).map(|r| r.stage.label())
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
    /// returns the initial dispatches (both proposers).
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

        let run = ShadowRun {
            run_id: run_id.clone(),
            main_agent_id: main_agent_id.clone(),
            main_prompt: main_prompt.clone(),
            mission_id: mission_id.clone(),
            prompt_msg_idx,
            stage: ShadowStage::Proposing,
            lanes,
            outputs: HashMap::new(),
        };

        let dispatches = vec![
            ShadowDispatch {
                agent_id: run.lanes["propose-a"].clone(),
                prompt: build_propose_prompt("A", &main_prompt),
                mission_id: mission_id.clone(),
                prompt_msg_idx: None,
            },
            ShadowDispatch {
                agent_id: run.lanes["propose-b"].clone(),
                prompt: build_propose_prompt("B", &main_prompt),
                mission_id,
                prompt_msg_idx: None,
            },
        ];

        self.runs.insert(main_agent_id, run);
        Some(dispatches)
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

        // Main agent finished → clean up shadow lanes and remove the run.
        if event_agent_id == run.main_agent_id {
            if matches!(run.stage, ShadowStage::Finalizing) {
                let lane_ids: Vec<String> = run.lanes.values().cloned().collect();
                self.runs.remove(&main_agent_id);
                cleanup_shadow_lanes(state, &lane_ids);
            }
            return Vec::new();
        }

        // Shadow finished: stash its output and maybe advance the stage.
        let role = role_of(run, event_agent_id);
        if let Some(role) = role {
            run.outputs.insert(role.clone(), message.to_string());
        }

        match run.stage {
            ShadowStage::Proposing => {
                if run.outputs.contains_key("propose-a") && run.outputs.contains_key("propose-b") {
                    run.stage = ShadowStage::Judging;
                    let judge_id = run.lanes["judge"].clone();
                    let prompt = build_judge_prompt(
                        &run.main_prompt,
                        run.outputs
                            .get("propose-a")
                            .map(String::as_str)
                            .unwrap_or(""),
                        run.outputs
                            .get("propose-b")
                            .map(String::as_str)
                            .unwrap_or(""),
                    );
                    let mission_id = run.mission_id.clone();
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
                    run.stage = ShadowStage::Reviewing;
                    let review_id = run.lanes["review"].clone();
                    let prompt = build_review_prompt(
                        &run.main_prompt,
                        run.outputs.get("judge").map(String::as_str).unwrap_or(""),
                    );
                    let mission_id = run.mission_id.clone();
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
                    run.stage = ShadowStage::Finalizing;
                    let main_id = run.main_agent_id.clone();
                    let prompt = build_final_prompt(run);
                    let mission_id = run.mission_id.clone();
                    let prompt_msg_idx = run.prompt_msg_idx;
                    return vec![ShadowDispatch {
                        agent_id: main_id,
                        prompt,
                        mission_id,
                        prompt_msg_idx,
                    }];
                }
            }
            ShadowStage::Finalizing => {}
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
        for (main_id, run) in self.runs.iter() {
            if run.lanes.values().any(|id| id == agent_id) {
                return Some(main_id.clone());
            }
        }
        None
    }
}

fn role_of(run: &ShadowRun, agent_id: &str) -> Option<String> {
    run.lanes
        .iter()
        .find(|(_, id)| id.as_str() == agent_id)
        .map(|(role, _)| role.clone())
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
     execute the actual work."
}

fn build_propose_prompt(variant: &str, user_prompt: &str) -> String {
    format!(
        "{awareness}\n\n\
         {readonly}\n\n\
         ## YOUR ROLE\n\
         You are Shadow-Proposer-{variant}, a hidden support agent drafting \
         one candidate approach for the following user request. Work \
         independently; do NOT coordinate with other proposers. Be concrete \
         and opinionated. Your proposal must respect nit's parsimony rules.\n\n\
         Deliverable (≤ 300 words, text only):\n\
         1. One-sentence summary of your approach.\n\
         2. Step-by-step plan, naming concrete file paths where possible.\n\
         3. Key tradeoffs or risks, including any tier/parsimony concerns.\n\n\
         User request:\n{user_prompt}",
        awareness = nit_system_awareness(),
        readonly = shadow_readonly_clause(),
    )
}

fn build_judge_prompt(user_prompt: &str, proposal_a: &str, proposal_b: &str) -> String {
    format!(
        "{awareness}\n\n\
         {readonly}\n\n\
         ## YOUR ROLE\n\
         You are Shadow-Judge, a hidden support agent. Two proposers drafted \
         independent approaches to the user's request. Compare them, pick \
         the stronger one (or synthesise a better hybrid), and produce a \
         SINGLE recommended plan. Reject any step that would violate nit's \
         parsimony rules.\n\n\
         Deliverable (≤ 300 words, text only):\n\
         1. Which proposal is stronger and why (one sentence).\n\
         2. The final recommended plan (numbered steps).\n\
         3. Explicit callouts: what was dropped from the weaker proposal \
         and why, including any parsimony/tier concerns.\n\n\
         User request:\n{user_prompt}\n\n\
         Proposal A:\n{proposal_a}\n\n\
         Proposal B:\n{proposal_b}",
        awareness = nit_system_awareness(),
        readonly = shadow_readonly_clause(),
    )
}

fn build_review_prompt(user_prompt: &str, judged_plan: &str) -> String {
    format!(
        "{awareness}\n\n\
         {readonly}\n\n\
         ## YOUR ROLE\n\
         You are Shadow-Reviewer, a hidden support agent. Stress-test the \
         judged plan below: look for missed edge cases, broken assumptions, \
         unstated dependencies, and concrete file paths the plan should \
         touch but doesn't. Flag anything that risks tripping nit's \
         parsimony detector or dropping the tier below III.\n\n\
         Deliverable (≤ 300 words, text only):\n\
         1. Risks / holes you found (bullets).\n\
         2. Suggested additions or corrections.\n\
         3. A short \"do / don't\" list for the executing agent, including \
         parsimony reminders.\n\n\
         User request:\n{user_prompt}\n\n\
         Judged plan:\n{judged_plan}",
        awareness = nit_system_awareness(),
        readonly = shadow_readonly_clause(),
    )
}

fn build_final_prompt(run: &ShadowRun) -> String {
    let empty = String::new();
    let propose_a = run.outputs.get("propose-a").unwrap_or(&empty);
    let propose_b = run.outputs.get("propose-b").unwrap_or(&empty);
    let judge = run.outputs.get("judge").unwrap_or(&empty);
    let review = run.outputs.get("review").unwrap_or(&empty);
    format!(
        "## SHADOW CONTEXT (hidden support agents)\n\
         The following analysis was produced by four hidden support agents \
         (propose-a, propose-b, judge, review) to help you answer the user. \
         Treat it as advisory context — override any suggestion you disagree \
         with, and cite the risks the reviewer surfaced when relevant. The \
         shadows have already been briefed on nit's tier ladder and \
         parsimony detector, so their plans should be quality-aware.\n\n\
         ### Proposal A\n{propose_a}\n\n\
         ### Proposal B\n{propose_b}\n\n\
         ### Judge's recommended plan\n{judge}\n\n\
         ### Reviewer's risks and corrections\n{review}\n\n\
         ## USER REQUEST\n{user_prompt}\n",
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/shadow.rs"]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_shadow_command_accepts_explicit_prefix() {
        let cmd = parse_shadow_command("@shadow refactor core").unwrap();
        assert_eq!(cmd.prompt, "refactor core");
    }

    #[test]
    fn parse_shadow_command_rejects_embedded_prefix() {
        assert!(parse_shadow_command("please @shadow foo").is_none());
        assert!(parse_shadow_command("@shadows foo").is_none());
        assert!(parse_shadow_command("@shadow").is_none());
    }

    #[test]
    fn parse_shadow_command_tolerates_leading_whitespace() {
        let cmd = parse_shadow_command("  @shadow do it").unwrap();
        assert_eq!(cmd.prompt, "do it");
    }

    #[test]
    fn should_auto_enable_shadows_triggers_on_keyword() {
        assert!(should_auto_enable_shadows("Refactor the widget module"));
        assert!(should_auto_enable_shadows("rewrite this function please"));
        assert!(should_auto_enable_shadows("Implement SSE streaming"));
    }

    #[test]
    fn should_auto_enable_shadows_triggers_on_length() {
        let long = "a".repeat(501);
        assert!(should_auto_enable_shadows(&long));
    }

    #[test]
    fn should_auto_enable_shadows_is_quiet_for_short_questions() {
        assert!(!should_auto_enable_shadows("what does this do?"));
        assert!(!should_auto_enable_shadows("fix typo"));
        assert!(!should_auto_enable_shadows("why is the test flaky?"));
    }

    #[test]
    fn shadow_lane_id_roundtrip() {
        let id = shadow_lane_id("codex", "01", "propose-a");
        assert_eq!(id, "codex#shadow-01-propose-a");
        let (base, run_id, role) = parse_shadow_lane_id(&id).unwrap();
        assert_eq!(base, "codex");
        assert_eq!(run_id, "01");
        assert_eq!(role, "propose-a");
    }

    #[test]
    fn parse_shadow_lane_id_handles_roles_with_dashes() {
        let id = "claude-main#shadow-07-propose-b";
        let (base, run_id, role) = parse_shadow_lane_id(id).unwrap();
        assert_eq!(base, "claude-main");
        assert_eq!(run_id, "07");
        assert_eq!(role, "propose-b");
    }
}
