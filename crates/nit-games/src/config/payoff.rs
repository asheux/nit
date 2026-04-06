use super::types::PayoffConfig;
use crate::game::PayoffMatrix;

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
        let r = cells[0][0].0;
        let s = cells[0][1].0;
        let t = cells[1][0].0;
        let p = cells[1][1].0;
        let symmetric =
            cells[0][0].1 == r && cells[0][1].1 == t && cells[1][0].1 == s && cells[1][1].1 == p;
        if symmetric {
            if let Some(value) = config.r {
                if value != r {
                    errors.push("payoff.R does not match payoff.matrix[0][0]".into());
                }
            }
            if let Some(value) = config.s {
                if value != s {
                    errors.push("payoff.S does not match payoff.matrix[0][1]".into());
                }
            }
            if let Some(value) = config.t {
                if value != t {
                    errors.push("payoff.T does not match payoff.matrix[1][0]".into());
                }
            }
            if let Some(value) = config.p {
                if value != p {
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

pub(super) fn fallback_payoff(config: PayoffConfig) -> PayoffMatrix {
    let r = config.r.unwrap_or(3);
    let s = config.s.unwrap_or(0);
    let t = config.t.unwrap_or(5);
    let p = config.p.unwrap_or(1);
    let matrix = [[[r, r], [s, t]], [[t, s], [p, p]]];
    PayoffMatrix::from_matrix(matrix)
}
