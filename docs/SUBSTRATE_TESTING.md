# Inspect coordination events

Use a temporary workspace to follow a coordination event from an agent through the TUI to its saved record. For repeatable checks independent of a model or account, run the integration tests below.

## Inspect a live workspace

1. Open the workspace in Nit. In Normal mode enter `:signals`. Check the sender, target, kind and generation of a record. Esc closes the overlay.
2. Use `:claims` to inspect file and region claims, including their remaining lifetime. Use `:assumptions` to inspect recorded dependencies. These views can be empty before agents report activity.
3. After a turn completes, compare the visible events with `.nit/substrate/state.json`. A recorded file write can invalidate overlapping assumptions. A write by an unrelated process may never reach the agent event stream.
4. For an unexpected retry, check Agent Ops → Diag for a file conflict, failure or corrective action. Compare the current diff with the agent's last artifact before rerunning the task.

For a read-only inspection from the workspace root, this prints the saved generation and record counts.

```sh
jq '{generation, signals: (.signals | length), claims: (.claims | length), assumptions: (.assumptions | length)}' .nit/substrate/state.json
```

The snapshot can lag the live view. Expiry uses completed-turn generations. The wall-clock sweep alone will not consume additional generations while the workspace is idle.

## Reproduce a specific behavior

From the Nit source checkout, these tests exercise the event handlers and the MCP server.

```sh
cargo test --locked -p nit-core file_write_auto_claim_conflict
cargo test --locked -p nit-core file_write_invalidates_overlapping_assumption
cargo test --locked -p nit-core tick_does_not_advance_generation
cargo test --locked -p nit-core mission_memory
cargo test --locked -p nit-mcp --test discovery --test tools_call
```

The first two cases distinguish conflicting access from an invalidated dependency. The third checks the distinction between elapsed time and completed work. The mission-memory tests exercise indexing and retrieval. The MCP tests use controlled requests and a local test connection, so they can run without a provider account.

## Diagnose a missing event

- **A skill opens but no coordination record appears.** The skill catalog is local to the tool server. Live signal and claim tools require the TUI backchannel. Verify that `nit-mcp-server` is installed beside `nit` and the workspace is running.
- **A claim never appears.** Check the tool's arguments, overlapping claims and resulting violation signals. A successful transport exchange only establishes that the request reached its receiver.
- **A warning disappeared.** Strength decays across generations. Expiry and pruning can remove a record from the live tables. Use the mission artifacts for a durable account of the task.
- **A previous mission was suggested.** Compare its title, touched paths and task summaries with the new request. Cross-mission retrieval ranks text and path overlap.

[Coordination records and tool arguments](SUBSTRATE.md) · [Reasons for retries](LIVING_SYSTEM.md)
