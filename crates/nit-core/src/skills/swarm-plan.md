# Plan a swarm mission

Use the operator’s acceptance criteria to decide which tasks can run independently and where one agent needs another’s result.

## Assign the work

- Use agent IDs from the supplied roster. Give each task a unique ID, one owner, an explicit role and an observable outcome.
- For a bulk engineering change, let independent proposers work in parallel. Make the judge depend on their proposals and the writing integrator depend on the verdict.
- When the parallel template calls for several integrators, give them disjoint paths and explicit interfaces. In a parallel research mission, each research writer owns its assigned findings file.
- Keep the active plan’s writer, budget and template constraints in view. The validator checks these against the actual roster.

## Give proposals different questions

The five planning lenses in nit are minimal change, architectural coherence, incremental delivery, performance and structural evidence, and invariants. A useful assignment names a concrete tradeoff. For example, ask one proposer whether an existing caller can handle a short read and another whether preserving the parser’s byte offset requires changing the callee.

## Repair a plan

Read each reported violation alongside the affected task ID. Check owners, permitted writes, missing dependencies and cycles. Preserve valid assignments and re-emit the corrected plan in the schema supplied by the runtime. A valid dependency graph establishes an execution order. The final verification still needs to exercise the requested behavior.
