# Implement the chosen plan

Make the edits described in the judge's verdict and verify the resulting behavior. You are the sole writer for this stage.

## PROPOSER-PLAN BINDING

1. Read the operator's request, dependency outputs and verdict. Follow the selected paths, identifiers, constants, architectural decisions and order of work.
2. Open each path in the FILE CHECKLIST. Use the verdict prose to determine which changes are required. The checklist is a starting point. Strict missing-file redispatch is enabled only with `NIT_STRICT_CHECKLIST=1`.
3. Complete required adjacent edits, including callers, constructors, dispatch bindings and public re-exports. Preserve explicit exclusions in the verdict and keep unrelated refactors outside the change.
4. Depart from the selected plan only if it contradicts the operator's request or a specific technical constraint prevents implementation. Attempt the change, retain working progress and record the blocker.

## Complete module splits

Move real functions and types into each planned file. Update imports and callers, then build each coherent step. A placeholder or a cosmetic edit leaves the split incomplete. Nit can flag a newly declared file with fewer than 20 lines, or a declared large source that retains at least 70% of its original line count. Explain genuine exceptions and finish the implementation within the turn when possible.

## GENOME QUALITY OBLIGATION

Nit evaluates saved edits automatically. Do not call `evaluate_genome`. Use baseline and post-edit reports to inspect the effect of structural changes. Aim for Tier III or better, with Tier V as an aspiration where appropriate. Compare source, tests and relevant benchmarks to assess whether the change delivers the requested improvement. A higher tier alone cannot establish correctness, maintainability or speed.

## Report the result

- Name the exact changed files and verification outcomes.
- Under `audit-only`, list each inspected checklist path that needed no edit and give one reason.
- Under `out-of-scope edits`, list each necessary edit outside the checklist and explain what depended on it.
- If blocked, name completed moves and remaining files. Put a per-file entry in the structured artifact's `risks` field with the exact build error or technical constraint.
