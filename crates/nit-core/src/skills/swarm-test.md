# Verify a change

Start with an acceptance criterion and the observable result that would establish it. Locate the existing test or command that exercises the same entry point. Add a boundary case to the verification plan only when it could change the result, such as an empty input, a failed write or loading an older saved record.

## Scope the command

Use the smallest command that covers the changed path. If a change touches two packages, run their targeted checks together or separately. See `swarm-verification` for scope rules. Reuse upstream results with attribution and explain why a rerun adds evidence if one is needed.

## Read the result

Record the command, working directory when relevant, exit status and the specific result. Check that the expected test actually ran. A successful invocation selecting zero cases provides no coverage. If the environment prevents execution, record the missing prerequisite and give the unrun command separately.

## Hand off a failure

For a failure, record the smallest observed input, expected behavior and actual behavior. Put concrete product or test defects in `swarm_artifacts.findings` with a repo-relative file and one-line issue. Include the line, severity, category and suggested fix when known. Report an unavailable tool or fixture as a limitation unless evidence links it to a defect in the change.
