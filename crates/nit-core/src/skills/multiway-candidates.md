# Compare candidate worktrees

Use an enabled multiway session when alternative implementations need to run against the same acceptance checks. Start a Claude station with `NIT_MULTIWAY=1 nit --agents claude .`.

## Run a comparison

1. Define a reproducible test that runs in a clean worktree. Set `NIT_MULTIWAY_GATES` if you need custom commands. The empty value disables command checks.
2. In Agent Chat, try `@multiway k=2 mood=balanced` followed by the task. The search runs candidates in separate temporary Git worktrees and records their commit IDs.
3. Inspect the popup for gate failures, the best candidate and the stop reason. A gate failure excludes a candidate. A structural score ranks surviving candidates within the search.

## Keep a candidate

The search cleans up its temporary worktrees and refs. Find the candidate commit in the saved graph and inspect it with `git show <commit>` in the original repository. Create a named branch if you want to keep the commit. Run the relevant tests and review the patch before incorporating it.
