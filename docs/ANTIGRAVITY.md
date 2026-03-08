# Antigravity (antigravity.google) — notes + ideas for nit

This doc is a quick reverse‑engineering pass of **Google Antigravity** from public materials, with
specific product ideas that map well onto nit’s existing Agent Station (`Agent Ops` + `Agent Chat`)
and security posture.

## What Antigravity is (high level)

From Google’s own description (public preview, Nov–Dec 2025), Antigravity is an **agentic
development platform**: it combines a familiar IDE/editor surface with a **manager/mission‑control
surface** for spawning and supervising multiple agents across workspaces.

Key product primitives:

- **Two primary surfaces**
  - **Editor View**: IDE-like editing with agent panel, tab completions, and inline commands.
  - **Manager Surface**: a dedicated agent-first interface to spawn/orchestrate/observe multiple
    agents working asynchronously across different workspaces.
- **Multi-surface autonomy**: agents can operate across **editor + terminal + browser**, and can
  plan/execute/verify end-to-end tasks without constant user intervention.
- **Artifacts over raw logs**: agents produce “Artifacts” (task lists, implementation plans,
  screenshots, browser recordings) designed to be reviewable at a glance.
- **Artifact-level feedback loop**: users can comment on artifacts (doc-style comments; also
  feedback on screenshots), and the agent incorporates the feedback without needing to stop.
- **Knowledge base**: agents can save useful context/snippets into a knowledge base to improve
  future work.
- **Model optionality**: supports multiple model providers (Gemini + Claude + GPT-OSS were
  explicitly called out in 2025 launch materials).

## How Antigravity’s tool integration works (MCP)

Antigravity leans heavily on **Model Context Protocol (MCP)** to give agents executable tools and
data access.

Observed design choices:

- **Built-in “MCP Store” UI** for discovering/installing servers, with **UI forms** for setup.
- **Credential handling**: setup flows can store credentials securely so agents can use tools
  without pasting secrets into chat.
- **A generated JSON config file**: docs reference a `mcp_config.json` that Antigravity updates
  automatically; there’s a “View raw config” entry point to edit/inspect the config.
- **Prebuilt servers (via MCP Toolbox for Databases)**: Google Cloud positions MCP Toolbox as a
  standardized server layer between IDE and services like BigQuery / AlloyDB / Spanner / Looker.

Example from Google Cloud docs (BigQuery via MCP Toolbox):

```json
{
  "mcpServers": {
    "bigquery": {
      "command": "npx",
      "args": ["-y", "@toolbox-sdk/server", "--prebuilt", "bigquery", "--stdio"],
      "env": { "BIGQUERY_PROJECT": "PROJECT_ID" }
    }
  }
}
```

## Ideas that map cleanly into nit

nit already has many of the same building blocks (missions, DAG view, MCP connection status, swarm
orchestration, gate monitor, sandbox/approval settings). The main gap vs. Antigravity is the
**artifact/feedback/knowledge** layer and the **MCP Store-like UX**.

### 1) “Artifacts” as a first-class UI + file format

Goal: turn “agent work” into reviewable deliverables, not scrolling chat transcripts.

Concrete proposal:

- Add a mission-scoped artifact store at `.nit/artifacts/<mission_id>/...` with types:
  - `plan.md` (task list + approach)
  - `diff.patch` or `diff.md` (human-readable + applyable)
  - `verify.md` (gates run + results summary)
  - `ui/` screenshots (`.png`) + small metadata (`.json`)
  - `walkthrough.md` (what changed + why)
- Add an `Agent Ops` tab (or repurpose the existing hidden `Evidence` tab) to browse artifacts by
  mission and open them in-place.
- Add quick actions: “Send feedback”, “Ask to revise”, “Apply patch”, “Re-run gates”.

### 2) Artifact-level feedback (comment threads)

Goal: let the operator correct 10–20% mistakes cheaply without rewriting the whole prompt.

Concrete proposal:

- Allow inline comments on text artifacts (line-anchored threads) stored as
  `.nit/artifacts/<mission_id>/<artifact>.comments.json`.
- A “feedback inbox” inside Agent Ops, similar to Antigravity’s “comment on doc” loop:
  - selecting a comment turns it into a structured follow-up prompt that the agent must address.
- For images: start simple (file-level comments), then later add coordinate-based annotations.

### 3) Knowledge base with human approval

Goal: reduce repeated “teach the agent the repo” overhead while staying safe-by-default.

Concrete proposal:

- A repo-local knowledge base at `.nit/knowledge/` (small Markdown “cards”).
- Add a “Propose knowledge item” flow:
  - agent suggests a knowledge card,
  - user approves/edits,
  - nit includes approved cards as context for future missions.
- Keep it explicit to preserve nit’s “no hidden background state” vibe.

### 4) MCP “Store” UX (without becoming a plugin platform)

Goal: make MCP servers discoverable and safe to run, without “random scripts”.

Concrete proposal:

- Extend `Agent Ops → MCP` to support **multiple MCP servers** (not just the Codex runtime), with:
  - a local config file (compatible with `mcp_config.json` shape),
  - “View raw config” and “Install from template” flows,
  - tool list + per-tool enable/disable (checkboxes), similar to other MCP clients.
- Curate templates that are “safe-ish” and transparent:
  - git, ripgrep, filesystem (already local)
  - optional: database toolbox (if user explicitly opts in)
- Secrets: integrate with OS keychain (or an encrypted file) and inject env vars at spawn time;
  never print secrets into Agent Chat.

### 5) Review policy UI (operator-controlled autonomy)

Antigravity surfaces “review policy” explicitly. nit already has Codex sandbox + approval policy,
but they’re mostly CLI flags today.

Concrete proposal:

- Add an in-UI toggle (per mission) mapping to:
  - sandbox level (read-only vs workspace-write),
  - approval policy (untrusted/on-failure/on-request/never),
  - optional “destructive ops require confirm” guardrail (even in `never`).

### 6) “Verify with evidence” gate bundles become artifacts

nit already has a Gate Monitor; we can make it more “artifact-like”:

- Each gate run emits a `verify.md` artifact with:
  - commands run, pass/fail, key output excerpts, timing.
- Gate failures show a small “What to try next” section (agent-proposed).

### 7) Browser-loop as an optional “harness”

Antigravity’s browser integration enables agents to verify UI flows.

Concrete proposal:

- Start as an **external tool** (Playwright) invoked via a dedicated “UI verify” gate bundle.
- Store screenshots/videos as artifacts and show them in the artifacts UI (even if nit can’t render
  images directly, it can open them externally and track them in the mission timeline).

## References (public)

- Google Developers Blog — *Build with Google Antigravity, our new agentic development platform*
  (Nov 20, 2025): https://developers.googleblog.com/en/build-with-google-antigravity-our-new-agentic-development-platform/
- Google Cloud Blog — *Connect your enterprise data to Google’s new Antigravity IDE* (Dec 15, 2025):
  https://cloud.google.com/blog/products/data-analytics/connect-google-antigravity-ide-to-googles-data-cloud-services
- Google Cloud Docs — *Connect LLMs to BigQuery with MCP* (includes Antigravity + `mcp_config.json`
  steps): https://docs.cloud.google.com/bigquery/docs/pre-built-tools-with-mcp-toolbox
- Firebase Docs — *Firebase MCP server* (Antigravity install path + `mcp_config.json` example):
  https://firebase.google.com/docs/cli/mcp-server
