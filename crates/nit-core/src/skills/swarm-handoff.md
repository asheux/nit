# Hand off evidence

A swarm task’s human-readable report and structured artifact block serve different readers. The report explains the outcome. The block lets nit and downstream agents locate the work.

## Fill the relevant fields

- Use the task ID and version from the active request. Put paths to intended changes in a proposer’s or judge’s `files` array. Paths read only for context belong in `notes`.
- Writers report actual edits in `diffs`. Include the path and a concise description of the behavioral change.
- Record commands you ran, what they exercised and their outcomes. Identify skipped checks and blockers explicitly.
- For test and review roles, actionable `findings` identify the file, line, severity, category, issue and suggested fix. Writers report their changes through `diffs` and `notes`.

## Close the task

Follow the JSON schema in the task prompt, include only relevant keys and preserve its completion sentinel. In Agent Ops, open **Artifacts** to inspect the parsed result beside the original output. If parsing failed, the original output can still contain evidence worth inspecting.
