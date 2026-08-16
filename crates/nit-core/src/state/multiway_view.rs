//! Routing payload handed from the chat submit to the multiway run loop.
//!
//! Phase 7 lets `@shadow` / `@swarm` / `@all` / bare chat reach the multiway
//! engine through a `mode=multiway` modifier. The chat submit records which
//! front-end the operator used and the cleaned command (the modifier stripped);
//! `app/runner.rs` — which owns the `MultiwayRuntime` — maps that source to a
//! search policy and starts the run. Carrying the source plus a cleaned command,
//! rather than a pre-built policy, keeps this type free of any `nit-multiway`
//! dependency: nit-multiway already depends on nit-core, so the reverse edge
//! would be a crate cycle.
//!
//! The Phase 6b render-only [`MultiwayView`] lives here for the same reason: it
//! is the plain-data projection of the live search the popup widget renders,
//! rebuilt each UI tick from the engine so the widget can own no state.

use crate::genome_report::GenomeTier;

/// Which dispatch front-end routed into the multiway engine.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MultiwaySource {
    /// Canonical `@multiway <task>`.
    Explicit,
    /// `@shadow mode=multiway <task>`.
    Shadow,
    /// `@swarm [N] [template=…] mode=multiway <task>`.
    Swarm,
    /// `@all mode=multiway <task>`.
    All,
    /// Bare chat `mode=multiway <task>` with no command prefix.
    Bare,
}

/// Roster-selectable multiway SEARCH mood — the nit-core-local mirror of
/// `nit_multiway::policy::Mood`. nit-core cannot name the engine enum, since
/// `nit-multiway → nit-core` is the only legal edge of the dependency graph; the
/// projection back onto the engine mood lives at the nit-tui boundary in
/// `multiway::dispatch::engine_mood`, twin of how `MultiwayNodeStatus` is built.
/// Unrelated to the substrate `nit_core::Mood` (exploration/consolidation).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultiwaySearchMood {
    Explore,
    #[default]
    Balanced,
    Exploit,
}

/// One immutable snapshot of the whole multiway search, rebuilt each UI tick and
/// streamed to the Phase 6b popup. It holds no engine handles, so the widget that
/// renders it owns no state and close/reopen is non-destructive.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MultiwayView {
    pub header: MultiwayHeader,
    /// Pre-order nodes (parent before children); `depth` drives indentation.
    pub nodes: Vec<MultiwayNodeView>,
    /// Node ids on the best/kept lineage, rendered in gold.
    pub kept_path: Vec<String>,
    /// Short id of the node the search will expand next. No turn is in flight
    /// mid-tick — the post-expand observer only sees committed nodes — so this
    /// marks the frontier pick, not a running turn.
    pub in_flight: Option<String>,
}

/// Search-wide metrics for the popup header: mood·k, turns/nodes against budget,
/// the best score/tier so far, and the live stop reason.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MultiwayHeader {
    pub mood: String,
    pub k: usize,
    pub turns: usize,
    pub max_turns: usize,
    pub nodes: usize,
    pub max_nodes: usize,
    pub frontier: usize,
    pub best_score: f32,
    pub best_tier: GenomeTier,
    pub stop_reason: Option<String>,
}

/// One node rendered as a single indented row in the tree.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MultiwayNodeView {
    pub id: String,
    pub short_id: String,
    pub depth: u16,
    pub status: MultiwayNodeStatus,
    pub score: Option<f32>,
    pub tier: Option<GenomeTier>,
    pub summary: String,
    /// Files the turn changed, rendered as `(N files)`.
    pub changed: usize,
    /// Parent ids; more than one marks a Phase-4 merge node.
    pub parents: Vec<String>,
}

/// Cross-boundary projection of `nit_multiway`'s node status — nit-core cannot
/// name the engine enum, so the runtime maps onto this when it builds the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MultiwayNodeStatus {
    Open,
    Expanded,
    GateFailed,
    Dominated,
    Held,
    Solution,
}

/// One pending multiway dispatch: the front-end that produced it and the
/// canonical command with `mode=multiway` already stripped, so the run loop can
/// re-parse it losslessly through that front-end's own parser.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingMultiway {
    pub source: MultiwaySource,
    pub command: String,
}
