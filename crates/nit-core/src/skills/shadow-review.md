# Compare two approaches

Use `@shadow` in Agent Chat for a decision that benefits from two independent proposals. Name the constraint that distinguishes a good solution, such as preserving a caller’s error behavior.

## Evaluate the proposals

- Advisory turns A and B each propose an approach without editing the checkout. Compare their file coverage, assumptions and expected behavior.
- The judge chooses or combines the approaches. The reviewer checks the chosen plan for omissions, edge cases and unnecessary work.
- The selected agent receives the advice and the original request for its final turn. Its normal writing permissions apply there.

## Inspect the result

Look for **Proposing**, **Judging**, **Reviewing** and **Finalizing** in chat. A complete pass uses five model turns. If an advisory turn fails, the main agent can receive the original request directly. Open the **REPLY** artifact and check the underlying diff or tests before adopting the result.
