use nit_core::GenomeTier;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Value {
    /// `true` ⇔ a hard build/test gate FAILED ⇒ the node is NON-VIABLE and is
    /// excluded from the frontier. `false` ⇔ all gates passed ⇒ viable. The
    /// field reads as "gated OUT" — do not invert the polarity.
    pub gated: bool,
    /// Comparable best-first heuristic in `[0.0, 1.0]`; higher is better. Never
    /// NaN (the valuer coerces NaN to 0.0). Order via [`f32::total_cmp`], never
    /// `partial_cmp`.
    pub score: f32,
    pub tier: GenomeTier,
}

impl Value {
    pub fn is_viable(&self) -> bool {
        !self.gated
    }
}
