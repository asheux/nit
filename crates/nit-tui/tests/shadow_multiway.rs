//! Phase 7 mapping: `@shadow mode=multiway` parses onto `ShadowCommand` and maps
//! to a `k = 2`, `Balanced` multiway policy whose mood admits the merge the
//! production `JudgeMergeOracle` (the shadow judge) resolves. The default
//! `@shadow` path must stay untouched — the byte-identical off-path the engine's
//! `NIT_MULTIWAY` rollout guarantees.

use nit_multiway::node::NodeId;
use nit_multiway::policy::{merge_candidates, Mood};
use nit_tui::shadow::{parse_shadow_command, shadow_multiway_policy, ShadowMode};

#[test]
fn parse_detects_mode_multiway_and_strips_the_token() {
    let cmd = parse_shadow_command("@shadow mode=multiway refactor the parser")
        .expect("mode=multiway is still a valid @shadow command");
    assert_eq!(cmd.mode, ShadowMode::Multiway);
    // The token is consumed: the body the search runs is the prose only.
    assert_eq!(cmd.prompt, "refactor the parser");
}

#[test]
fn parse_leaves_default_shadow_path_unchanged() {
    // No modifier: same prompt and Default mode as before Phase 7.
    let plain = parse_shadow_command("@shadow refactor the parser").expect("plain @shadow");
    assert_eq!(plain.mode, ShadowMode::Default);
    assert_eq!(plain.prompt, "refactor the parser");

    // `mode=multiway` counts only as the leading modifier — mid-prompt it is
    // ordinary task text, so a search is never silently triggered.
    let embedded =
        parse_shadow_command("@shadow switch the mode=multiway flag").expect("embedded token");
    assert_eq!(embedded.mode, ShadowMode::Default);
    assert_eq!(embedded.prompt, "switch the mode=multiway flag");
}

#[test]
fn policy_is_k2_balanced_with_judge_merge_wiring() {
    let cmd = parse_shadow_command("@shadow mode=multiway tidy the module").expect("multiway cmd");
    let (policy, task) = shadow_multiway_policy(&cmd);

    // propose-a/-b are the first fork → k = 2 (clamped to the fd ceiling, which
    // on any normal host is far above 2).
    assert_eq!(policy.k, 2);
    // Balanced is the mood that admits a merge; the production JudgeMergeOracle
    // (the shadow judge) resolves it. Prove the mood actually reaches a merge.
    assert_eq!(policy.mood, Mood::Balanced);
    let siblings = [
        (NodeId::new("fork-a"), 1.0_f32),
        (NodeId::new("fork-b"), 0.5_f32),
    ];
    assert!(
        merge_candidates(policy.mood, &siblings).is_some(),
        "the shadow policy's mood must admit the merge the shadow-judge oracle resolves"
    );

    // The task body is mapped through verbatim under the writer role — the fork
    // turns edit worktrees, like the `@multiway` integrator.
    assert_eq!(task.prompt, "tidy the module");
    assert_eq!(task.role, "integrate");
}
