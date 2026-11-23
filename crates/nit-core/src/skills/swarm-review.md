# Review a change

Trace the change from the user's entry point to its observable result. Read the original request, the judge's acceptance criteria, the diff and the integrator's verification results. A path the producer missed can matter more than a file with a large diff.

## Follow the user-visible path

Follow changed values through their callers. For a command, check who can invoke it, what arguments reach the handler and what the user sees on failure. For persisted data, consider a previously saved record. For a retry, examine the state immediately after the first failure. Pick cases the change actually affects.

## Interpret genome changes

When a genome report indicates a regression, cite the file, encoder and measured change. A `complexity_field` increase can identify a function worth inspecting. Explain which branch or repeated responsibility makes it harder to maintain and suggest a specific edit. The `genome-encoders` skill explains the four measurements.

## Write a finding

Report each concrete fix in `swarm_artifacts.findings`. Include the repo-relative file, issue, relevant line, severity, category and a suggestion when available. Describe the triggering input and consequence. For example, “Pressing Enter on an empty selection indexes row zero and closes the dialog” tells the writer what to reproduce. Keep subjective preferences and unverified concerns in notes. If no issue surfaced, say what you examined and identify any material gap.
