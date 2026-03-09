# Antigravity (antigravity.google) — nit mapping and next moves

This is a practical reverse-engineering pass of **Google Antigravity** from public materials,
translated into nit terms. The goal is not to copy the product surface literally; it is to identify
the parts that fit nit’s terminal-first agent station and security model.

As of March 9, 2026, nit already implements more of this shape than the first draft of this doc
captured. The biggest change since then is that swarm artifacts are now both:

- persisted under `.nit/swarm/<mission-id>/...`, and
- surfaced in **Agent Ops → Artifacts** instead of being hidden behind legacy `Evidence` plumbing.

## What Antigravity is (high level)

From Google’s public preview materials (Nov–Dec 2025), Antigravity is an **agentic development
platform** with two core operator surfaces:

- **Editor View**: IDE/editor with inline completions, commands, and agent assistance.
- **Manager Surface**: a mission-control view for spawning, observing, and steering multiple agents
  asynchronously.

The product ideas that matter most for nit are:

- multi-agent orchestration across workspaces
- explicit autonomy/review controls
- structured artifacts instead of raw logs
- artifact-level feedback loops
- MCP/tool discovery and credential setup
- long-lived knowledge capture

## Current nit status vs. Antigravity

### Already strong

- **Mission control**: nit already has `Agent Chat`, `Agent Ops`, missions, swarm orchestration,
  planner/integrator/judge roles, DAG validation, mission-scoped clones, and gate execution.
- **Artifact schema**: swarm tasks can emit structured `swarm_artifacts` JSON with files, diffs,
  commands, risks, and notes.
- **Artifact persistence**: nit writes mission data under `.nit/swarm/<mission-id>/`, including
  `run.json`, `summary.json`, task `artifacts.json`, task `output.md`, gate `report.json`, gate
  `output.txt`, and now `gates/verify.md`.
- **Artifact UI**: `Agent Ops → Artifacts` now shows parsed task artifacts, missing-artifact
  warnings, output paths, and verification status for the selected mission.
- **Model optionality**: nit already supports local/mock, Codex, and Claude-backed lanes.
- **Operator-controlled safety**: sandbox and approval settings already exist through Codex runtime
  configuration.

### Partial

- **Review policy UX**: the controls exist, but mostly as CLI/runtime config rather than an obvious
  per-mission UI.
- **MCP UX**: nit exposes runtime connection status and reconnect/start/stop, but not an
  Antigravity-style MCP template store or guided config editor.
- **Verification artifacts**: nit now stores readable verification summaries, but not richer
  evidence like screenshots, timing charts, or “next actions” summaries.

### Missing

- **Artifact comments / feedback threads**
- **Approved repo-local knowledge cards**
- **MCP template/install flows**
- **Browser-loop harness surfaced as a first-class mission tool**
- **Cross-mission artifact compare/replay UI**

## nit-specific direction

The right move for nit is not “become Antigravity in a terminal.” It is:

1. keep mission state explicit and repo-local
2. make artifacts cheap to review
3. add feedback loops without hidden background state
4. keep tool setup transparent and auditable

That leads to the following priority order.

## Priority improvements

### 1) Artifacts as the default review surface

Status: **now partially shipped**

What nit has now:

- `swarm_artifacts` JSON emitted by tasks
- persistence under `.nit/swarm/<mission-id>/...`
- `Agent Ops → Artifacts`
- readable `gates/verify.md`
- Agent Chat keeps the transcript compact by hiding full agent replies; it shows `done (see ARTIFACTS)` and expects review to happen in the Artifacts popup.

What to add next:

- allow opening artifact files directly from the Artifacts tab
- add mission-level “walkthrough” and “implementation summary” markdown artifacts
- add quick actions beside artifacts: re-run gates, ask for revision, send to integrator

Important nit choice:

- keep the canonical store under `.nit/swarm/<mission-id>/...`
- do **not** invent a second artifact root unless there is a compelling schema reason

### 2) Artifact-level feedback threads

Status: **missing**

Best nit shape:

- store comments beside artifacts, e.g.
  `.nit/swarm/<mission-id>/tasks/<task-id>/artifacts.comments.json`
- start with file-level and line-anchored text comments
- turn a selected comment into a structured follow-up prompt for the responsible task/agent

This matches Antigravity’s “comment on the artifact, not the whole transcript” loop without adding
opaque state.

### 3) Knowledge cards with human approval

Status: **missing**

Best nit shape:

- repo-local cards under `.nit/knowledge/`
- each card is plain Markdown plus a tiny metadata header
- agents may propose a card, but it is only activated after operator approval/edit

This keeps nit’s current “explicit local state” posture while reducing repeated repo-explaining.

### 4) MCP templates, not a plugin marketplace

Status: **partial**

nit should lean into a narrow, auditable version of Antigravity’s MCP store idea:

- guided templates for a small curated set of MCP servers
- visible generated config
- no hidden secret pasting into chat
- per-server enable/disable and tool listing

The right product is “transparent templates + keychain/env injection”, not an open-ended plugin
platform.

### 5) Review policy as a mission-scoped UI control

Status: **partial**

Antigravity is right to surface autonomy/review policy explicitly. nit should expose the existing
concepts per mission:

- sandbox level
- approval policy
- destructive-op confirmation guardrail

The important nit constraint is that the selected policy must be obvious in Agent Ops and recorded
in mission provenance.

### 6) Verification with richer evidence

Status: **partial**

Current nit output:

- gate bundle selection
- gate report JSON
- raw gate output
- human-readable `verify.md`

Next additions that would matter:

- per-gate timing
- operator-facing “what failed / what to try next”
- optional UI evidence attachments (screenshots, recordings, external files)

### 7) Browser loop as a gate bundle, not a magic background agent

Status: **missing**

The most nit-native way to absorb Antigravity’s browser loop is:

- add a dedicated browser verification gate bundle
- use Playwright or similar as an explicit external tool
- store screenshots/videos under the same mission artifact tree

That keeps browser work inspectable and replayable.

## Recommended near-term roadmap

1. Add artifact quick actions in `Agent Ops → Artifacts`.
2. Add comment threads for text artifacts.
3. Add repo-local approved knowledge cards.
4. Add guided MCP templates and raw-config editing.
5. Add per-mission review-policy UI.
6. Add browser verification as an optional gate bundle.

## References (public)

- Google Developers Blog — *Build with Google Antigravity, our new agentic development platform*
  (Nov 20, 2025): https://developers.googleblog.com/en/build-with-google-antigravity-our-new-agentic-development-platform/
- Google Cloud Blog — *Connect your enterprise data to Google’s new Antigravity IDE* (Dec 15, 2025):
  https://cloud.google.com/blog/products/data-analytics/connect-google-antigravity-ide-to-googles-data-cloud-services
- Google Cloud Docs — *Connect LLMs to BigQuery with MCP*:
  https://docs.cloud.google.com/bigquery/docs/pre-built-tools-with-mcp-toolbox
- Firebase Docs — *Firebase MCP server*:
  https://firebase.google.com/docs/cli/mcp-server
