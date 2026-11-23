# What triggers an automatic agent retry?

When an agent unexpectedly retries a task, open Agent Ops → Diag and enter `:signals`. Find the warning or intervention for that agent, check which file or task it names, then compare it with the latest output in Agent Ops → Artifacts. Nit bases these decisions on observed events. A warning can identify a coordination problem even when the code itself is correct.

| What Nit observed | What can happen next | What to check |
|---|---|---|
| Two agents declared incompatible access to overlapping files or regions | A claim violation can trigger a retry. Repeated conflicts can prompt one agent to yield the resource. | Inspect `:claims`, assign one writer and review the actual diff. |
| Several recent warnings concern the same agent | Nit can ask that agent to stop retrying the same approach and identify its blocker. | Read the first failure and its command output. |
| A planner repeatedly names missing prerequisite tasks | Nit can ask the planner to correct the dependency list. | Open Agent Ops → DAG and compare task IDs with their prerequisites. |
| A file changes while an agent relies on a recorded assumption about it | Nit removes the assumption and posts a warning for its author. | Recheck the fact against the changed file. |

The source of a signal helps explain the action. Workers perform the task. Observers summarize patterns such as repeated failures. Arbiters use those observations to request a corrective action. Cooldowns and retry limits bound automatic redispatches. Once a budget is exhausted, a signal may remain without another turn starting.

The top bar uses **mood** for the current coordination policy. Exploration, Consolidation and Defensive change thresholds, expiry multipliers and how often the workspace is checked. These labels describe scheduling behavior. They do not measure developer sentiment, correctness or the probability of success.

File claims are advisory. An external agent can finish a write before Nit receives its event. Cancelling or retrying a task leaves any existing file changes on disk for review.

For the recorded fields, agent tools and file locations, see [Workspace coordination](SUBSTRATE.md). For a repeatable check that distinguishes an observation from an actual edit, see [Inspect coordination events](SUBSTRATE_TESTING.md).
