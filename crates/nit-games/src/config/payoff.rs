use super::types::PayoffConfig;
use crate::game::PayoffMatrix;

/// Builds a [`PayoffMatrix`] from a [`PayoffConfig`].
///
/// When a full 2x2 matrix is provided it is validated for correct dimensions
/// and, for symmetric games, cross-checked against any explicit R/S/T/P
/// scalars.  If the matrix is missing or malformed the function falls back to
/// [`fallback_payoff`], constructing a symmetric matrix from the scalar fields
/// (or their standard Prisoner's Dilemma defaults).
pub(super) fn payoff_from_config(config: PayoffConfig, errors: &mut Vec<String>) -> PayoffMatrix {
    if let Some(matrix) = config.matrix.as_ref() {
        if matrix.len() != 2 {
            errors.push("payoff.matrix must have 2 rows".into());
            return fallback_payoff(config);
        }
        let mut cells = [[(0i32, 0i32); 2]; 2];
        for (row_idx, row) in matrix.iter().enumerate() {
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
        let reward = cells[0][0].0;
        let sucker = cells[0][1].0;
        let temptation = cells[1][0].0;
        let punishment = cells[1][1].0;
        let symmetric = cells[0][0].1 == reward
            && cells[0][1].1 == temptation
            && cells[1][0].1 == sucker
            && cells[1][1].1 == punishment;
        if symmetric {
            if let Some(value) = config.r {
                if value != reward {
                    errors.push("payoff.R does not match payoff.matrix[0][0]".into());
                }
            }
            if let Some(value) = config.s {
                if value != sucker {
                    errors.push("payoff.S does not match payoff.matrix[0][1]".into());
                }
            }
            if let Some(value) = config.t {
                if value != temptation {
                    errors.push("payoff.T does not match payoff.matrix[1][0]".into());
                }
            }
            if let Some(value) = config.p {
                if value != punishment {
                    errors.push("payoff.P does not match payoff.matrix[1][1]".into());
                }
            }
        }
        let matrix = [
            [
                [cells[0][0].0, cells[0][0].1],
                [cells[0][1].0, cells[0][1].1],
            ],
            [
                [cells[1][0].0, cells[1][0].1],
                [cells[1][1].0, cells[1][1].1],
            ],
        ];
        PayoffMatrix::from_matrix(matrix)
    } else {
        fallback_payoff(config)
    }
}

/// Constructs a symmetric [`PayoffMatrix`] from the scalar R/S/T/P fields in
/// the config, falling back to standard Prisoner's Dilemma defaults
/// (R=3, S=0, T=5, P=1) for any missing value.
pub(super) fn fallback_payoff(config: PayoffConfig) -> PayoffMatrix {
    let reward = config.r.unwrap_or(3);
    let sucker = config.s.unwrap_or(0);
    let temptation = config.t.unwrap_or(5);
    let punishment = config.p.unwrap_or(1);
    let matrix = [
        [[reward, reward], [sucker, temptation]],
        [[temptation, sucker], [punishment, punishment]],
    ];
    PayoffMatrix::from_matrix(matrix)
}
