# Coordinate overlapping edits

Use claims and assumptions when agents share a checkout. They describe intended access and dependencies. They are advisory, and other processes can still modify the same file.

## Before writing

1. Open `:claims` to see the claimed path or inclusive line range, owner and remaining lifetime.
2. If the planned edit overlaps another writer, agree on ownership, narrow the range or wait for the dependency to finish.
3. Record assumptions about a caller, supported format or function signature. Include the file or region on which each assumption depends.

## After a change

Review `:signals` for reported events and `:assumptions` for invalidated dependencies. Re-read the actual source and retest the affected behavior. A signal’s strength controls its priority and retention. It does not measure the probability that a claim is true.

MCP coordination tools require a connection to a running station. When the connection is unavailable, include ownership and dependencies in the task handoff.
