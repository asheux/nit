use super::types::PayoffConfig;
use crate::game::PayoffMatrix;

/// When a full 2x2 matrix is provided, validates dimensions and cross-checks
/// against explicit R/S/T/P scalars for symmetric games. Falls back to scalar
/// construction on malformed input.
pub(super) fn payoff_from_config(config: PayoffConfig, errors: &mut Vec<String>) -> PayoffMatrix {
    let Some(raw_matrix) = config.matrix.as_ref() else {
        return fallback_payoff(config);
    };
    if raw_matrix.len() != 2 {
        errors.push("payoff.matrix must have 2 rows".into());
        return fallback_payoff(config);
    }
    let mut cells = [[(0i32, 0i32); 2]; 2];
    for (row_idx, row) in raw_matrix.iter().enumerate() {
        if row.len() != 2 {
            errors.push(format!("payoff.matrix row {row_idx} must have 2 columns"));
            return fallback_payoff(config);
        }
        for (col_idx, cell) in row.iter().enumerate() {
            if cell.len() != 2 {
                errors.push(format!(
                    "payoff.matrix cell [{row_idx}][{col_idx}] must have 2 entries"
                ));
                return fallback_payoff(config);
            }
            cells[row_idx][col_idx] = (cell[0], cell[1]);
        }
    }

    let derived = [cells[0][0].0, cells[0][1].0, cells[1][0].0, cells[1][1].0];
    let is_symmetric = cells[0][0].1 == derived[0]
        && cells[0][1].1 == derived[2]
        && cells[1][0].1 == derived[1]
        && cells[1][1].1 == derived[3];

    if is_symmetric {
        let scalar_overrides = [
            (config.r, derived[0], "R", "[0][0]"),
            (config.s, derived[1], "S", "[0][1]"),
            (config.t, derived[2], "T", "[1][0]"),
            (config.p, derived[3], "P", "[1][1]"),
        ];
        for (explicit, from_matrix, label, cell_ref) in scalar_overrides {
            if let Some(val) = explicit {
                if val != from_matrix {
                    errors.push(format!(
                        "payoff.{label} does not match payoff.matrix{cell_ref}"
                    ));
                }
            }
        }
    }

    let unpack = |c: (i32, i32)| [c.0, c.1];
    PayoffMatrix::from_matrix([
        [unpack(cells[0][0]), unpack(cells[0][1])],
        [unpack(cells[1][0]), unpack(cells[1][1])],
    ])
}

/// Symmetric matrix from scalar R/S/T/P fields, defaulting to PD (R=3, S=0, T=5, P=1).
pub(super) fn fallback_payoff(config: PayoffConfig) -> PayoffMatrix {
    let reward = config.r.unwrap_or(3);
    let sucker = config.s.unwrap_or(0);
    let temptation = config.t.unwrap_or(5);
    let punishment = config.p.unwrap_or(1);
    PayoffMatrix::from_matrix([
        [[reward, reward], [sucker, temptation]],
        [[temptation, sucker], [punishment, punishment]],
    ])
}
