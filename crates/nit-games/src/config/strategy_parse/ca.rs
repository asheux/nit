use super::super::types::{StrategyConfig, StrategySpecKind};
use crate::strategy::math::checked_pow_u128;

pub(in crate::config) fn normalize_ca_kind(
    raw: &StrategyConfig,
    errors: &mut Vec<String>,
) -> StrategySpecKind {
    let id = raw.id.as_str();
    let n = raw.n.unwrap_or(0) as u64;
    let k_raw = raw.k.unwrap_or(2);
    let k = k_raw.clamp(2, u8::MAX as usize) as u8;
    if k_raw < 2 {
        errors.push(format!("strategy '{id}': ca.k must be >= 2"));
    }
    if k_raw > u8::MAX as usize {
        errors.push(format!("strategy '{id}': ca.k must be <= {}", u8::MAX));
    }
    let r_raw = raw.r.unwrap_or(-1.0);
    let two_r = match neighborhood_radius_to_diameter(r_raw) {
        Some(value) => value,
        None => {
            errors.push(format!(
                "strategy '{id}': ca.r must satisfy r >= 0 and IntegerQ[2r]"
            ));
            0
        }
    };
    let t = raw.t.or(raw.steps).unwrap_or(10);
    if t == 0 {
        errors.push(format!("strategy '{id}': ca.t must be > 0"));
    }

    let neighborhood = two_r.saturating_add(1) as u32;
    if let Some(table_len) = checked_pow_u128(k as u128, neighborhood) {
        if table_len > 1_000_000 {
            errors.push(format!(
                "strategy '{id}': ca rule table too large ({table_len} entries), reduce k or r"
            ));
        }
    } else {
        errors.push(format!(
            "strategy '{id}': ca rule table size overflow for k={k} r={}",
            two_r as f32 / 2.0
        ));
    }

    StrategySpecKind::Ca {
        n,
        k,
        r: two_r as f32 / 2.0,
        t,
    }
}

/// Converts CA neighbourhood radius `r` to diameter `2r` as an integer.
/// Returns `None` if `r` is non-finite, negative, or `2r` is non-integer.
fn neighborhood_radius_to_diameter(r: f32) -> Option<u32> {
    if !r.is_finite() || r < 0.0 {
        return None;
    }
    let doubled = r * 2.0;
    let rounded = doubled.round();
    if (doubled - rounded).abs() > 1e-6 || rounded < 0.0 {
        return None;
    }
    Some(rounded as u32)
}
