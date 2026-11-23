# Coordinate agents working in the same workspace

When two agents may touch the same file, give each a clear scope. Nit's **substrate** records what agents have reported and which resource each expects to use. From Normal mode, enter `:claims` to see declared access, `:assumptions` to inspect recorded dependencies, or `:signals` to see recent events. Tab changes views. Use the arrow keys to move through the entries and Esc to close.

| View | What an entry means | Example |
|---|---|---|
| Signals | An agent or Nit reported an event about a file, an agent or the workspace. Strength affects display and retention. | A warning identifies a failing command. |
| Claims | An agent expects a particular kind of access for a limited number of generations. | A writer claims lines 100–180 in a parser. |
| Assumptions | An agent recorded a fact that depends on a file or region. | A caller assumes a parser function has a particular signature. |

Signals fade as completed turns advance the generation counter. Different kinds fade at different rates. Initial strength and current **mood** also affect retention. Strength is a priority weight, not a calibrated confidence score. A wall-clock maintenance pass prunes old entries and checks for interventions. Waiting for a few seconds does not itself advance the generation.

## How file conflicts work

Claims can target the workspace, a file or an inclusive line range. A workspace claim overlaps all resources. A file claim overlaps every region of that file. Two ranges on the same file overlap if they share a line. For different claimants, an exclusive write conflicts with an overlapping exclusive write, shared read or append-only claim. Shared reads and append-only claims can coexist with each other. Soft claims are compatible with every kind.

An observed file write can create an exclusive claim. A conflicting write can emit a violation and queue a corrective prompt. The write may already be on disk. Inspect the diff before assigning further work. When an observed write overlaps a recorded assumption, Nit removes the assumption and alerts its author. The payload of an assumption is opaque to Nit, so the warning asks the agent to recheck it.

## Give an agent the relevant operating instructions

NitSkills are short built-in reference documents. List them in a shell or open one directly.

```sh
nit skills
nit skills swarm-integrate
nit skills genome-quality
```

In Agent Chat, enter `/skills` or `/skills swarm-integrate` for the same references without starting a model turn. The catalog includes the propose, judge, integrate and verification roles, plus the genome encoders and quality measures. Each agent can read the relevant reference on demand. For structural metrics, interpret the result alongside the actual diff, tests and measured behavior. A higher simulation tier alone does not establish a better implementation. [Code encoders and scores](SEEDS.md)

## MCP tools

The Codex runner can expose these tools through the bundled `nit-mcp-server`. The connection from Codex to these tools is separate from Nit's choice of Codex turn transport (`auto`, `exec` or `mcp`).

| Tool | Inputs | Result |
|---|---|---|
| `nit_skill` | Optional `name` | List the catalog or return one reference. |
| `emit_signal` | `kind`, `target`, optional `payload` and `strength` | Record a lead, warning or other supported signal. |
| `assert_claim` | `kind`, `target`, `ttl_gens`, `rationale` | Request temporary access and check compatibility. |
| `assert_assumption` | `target`, `ttl_gens`, `rationale`, optional `fact` | Track a dependency until expiry or an overlapping write. |

For example, a worker can pass this tool call before changing a parser. The time to live is counted in generations and the coordination policy can adjust it.

```json
{
  "name": "assert_claim",
  "arguments": {
    "kind": "exclusive_write",
    "target": { "kind": "file", "path": "src/parser.rs" },
    "ttl_gens": 5,
    "rationale": "I am changing how this parser reports truncated records."
  }
}
```

Nit looks for the server next to the running `nit` executable. Live coordination writes require a running TUI and its local backchannel. The TUI listener uses Unix sockets. Skill lookup can work independently of that live connection. Attribution follows the agent or persistent MCP session identity supplied when Nit launches the tool server.

## Saved state and previous missions

Nit stores current coordination state at `.nit/substrate/state.json`. Missing or unreadable state can produce an empty view. Swarm reports live under `.nit/swarm/<mission-id>/`. Completed missions are indexed in `.nit/memory/index.json`. The planner can receive up to three prior missions selected by overlap in words and paths, with extra weight for uncommon terms. A matching title or path suggests a report to read. Check whether its conclusions still apply to the current revision.

[Inspect coordination events](SUBSTRATE_TESTING.md) · [Automatic retries](LIVING_SYSTEM.md) · [Swarm missions](SWARM.md)
