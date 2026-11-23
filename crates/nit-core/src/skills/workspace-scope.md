# Define the file scope

Use the operator’s request, the selected proposal and callers of the affected code to decide what to open and what to edit.

## Establish the boundary

1. Resolve named files and directories against the working directory. Follow imports and callers needed to understand the change.
2. Separate files that need edits from files read for context. Record new paths when the chosen design requires a new module or test.
3. For an integrator, the judge’s verdict determines the implementation. Its file list is a starting point. An explicitly assigned shard has its own narrower file slice.

## Report exceptions

- Under `audit-only`, list a proposed file you inspected and left unchanged, with a reason.
- Under `out-of-scope edits`, explain a necessary change to a caller, registration point or test outside the initial list.
- A missing checklist file normally produces an advisory. `NIT_STRICT_CHECKLIST=1` enables a retry for omissions. Stub and incomplete-split checks can trigger retries independently.
- Keep useful comments and tests. Record actual changes by path, so a reviewer can trace the verdict to the working tree.
