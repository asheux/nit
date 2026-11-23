HOW YOU ARE MEASURED:
Your code is evaluated across four encoders. Each captures a different dimension of code quality. Cross-encoder consistency measures how much they agree — low consistency means some dimensions are strong but others are weak. Your tier is determined by a soft bottleneck of the AST-driven encoders — the weakest encoder matters most, but strong performance on other encoders provides a modest lift. Focus on balanced, natural code rather than obsessing over one encoder.

ATTRIBUTION — what can and cannot move your score:
The encoders project source through tree-sitter and DROP comment, doc-string, and attribute subtrees before measurement. They chunk by AST node index, not source line. Consequence: comment adds or trims, doc-string edits, blank-line shuffles, and identifier renames that preserve AST shape CANNOT move tier, generations-survived, or cross-encoder consistency. If a retry prompt reports DEGRADED and the most visible change in your diff is a comment edit, the actual cause is a semantic change elsewhere in the same edit pass — look at control flow, function boundaries, type signatures, and expression structure. Recommendation line numbers (e.g., `fn_name:120-180`) shift when you add or remove lines above them; a shifted location is not a new finding. Do NOT revert comment changes hoping to fix the regression; reverting comments removes signal humans rely on and will not move the score.

ENCODER GUIDE (what each measures → how to improve naturally):

AST-driven encoders (determine the overall tier):
  token_spectrum — token semantic role distribution (keywords, operators, identifiers, literals, comments).
    → Write code with natural variety. Avoid long repetitive blocks of similar tokens. Do NOT add comments to boost this encoder — comments that exist only for score inflation are detected and penalized by the parsimony system.
  ast_structure — syntactic tree shape (nesting depth, branching factor, span size, node type variety).
    → Use appropriate abstraction boundaries. Reduce deep nesting with early returns. A mix of types (structs, enums, fns) emerges naturally from good design — do not add types just for variety.
  complexity_field — spatial heatmap of cyclomatic complexity, nesting depth, token entropy, and identifier uniqueness.
    → Keep cyclomatic complexity reasonable per function (aim for <= 8). Use descriptive names. Distribute logic across well-motivated functions.

Hybrid encoder (AST-aware, whitespace-filtered):
  structural — operates on semantic token roles from tree-sitter.
    → Naturally varied code scores well. Different function shapes emerge from solving different sub-problems — not from artificially varying signatures or padding with comments.

TARGETS (guidelines, not hard requirements to engineer toward):
- Tier III+ (Spaceship) on all AST encoders.
- Cyclomatic complexity <= 8 per function.
- Nesting depth <= 3 on average.
- Cross-encoder consistency >= 0.50.
These are outcomes of good code, not specifications to engineer toward.

nit measures quality automatically after your changes are written to disk. Do NOT call [evaluate_genome] — nit evaluates externally and will retry your turn with specific feedback if quality degrades. Focus on writing good code; if tier drops below III, nit will tell you exactly what to fix.

SMALL FILES: Files with fewer than 20 significant lines (lib.rs, mod.rs, re-export files) receive an automatic Tier III pass. Do NOT pad these files with unnecessary code, enums, helpers, or doc comments just to boost genome scores. Keep small files minimal and clean.
