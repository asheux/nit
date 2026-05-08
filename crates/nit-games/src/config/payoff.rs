use super::types::PayoffConfig;
use crate::game::PayoffMatrix;

const DEFAULT_R: i32 = 3;
const DEFAULT_S: i32 = 0;
const DEFAULT_T: i32 = 5;
const DEFAULT_P: i32 = 1;

pub(super) fn payoff_from_config(config: PayoffConfig, errors: &mut Vec<String>) -> PayoffMatrix {
    let Some(raw_matrix) = config.matrix.as_ref() else {
        return fallback_payoff(config);
    };
    let Some(cells) = parse_payoff_cells(raw_matrix, errors) else {
        return fallback_payoff(config);
    };

    let [[cc, cd], [dc, dd]] = cells;
    let derived = [cc.0, cd.0, dc.0, dd.0];
    let is_symmetric =
        cc.1 == derived[0] && cd.1 == derived[2] && dc.1 == derived[1] && dd.1 == derived[3];

    if is_symmetric {
        check_scalar_overrides(&config, derived, errors);
    }

    PayoffMatrix::from_matrix([[[cc.0, cc.1], [cd.0, cd.1]], [[dc.0, dc.1], [dd.0, dd.1]]])
}

fn parse_payoff_cells(
    raw_matrix: &[Vec<Vec<i32>>],
    errors: &mut Vec<String>,
) -> Option<[[(i32, i32); 2]; 2]> {
    if raw_matrix.len() != 2 {
        errors.push("payoff.matrix must have 2 rows".into());
        return None;
    }
    let mut cells = [[(0i32, 0i32); 2]; 2];
    for (row_idx, row) in raw_matrix.iter().enumerate() {
        if row.len() != 2 {
            errors.push(format!("payoff.matrix row {row_idx} must have 2 columns"));
            return None;
        }
        for (col_idx, cell) in row.iter().enumerate() {
            if cell.len() != 2 {
                errors.push(format!(
                    "payoff.matrix cell [{row_idx}][{col_idx}] must have 2 entries"
                ));
                return None;
            }
            cells[row_idx][col_idx] = (cell[0], cell[1]);
        }
    }
    Some(cells)
}

fn check_scalar_overrides(config: &PayoffConfig, derived: [i32; 4], errors: &mut Vec<String>) {
    let overrides = [
        (config.r, derived[0], "R", "[0][0]"),
        (config.s, derived[1], "S", "[0][1]"),
        (config.t, derived[2], "T", "[1][0]"),
        (config.p, derived[3], "P", "[1][1]"),
    ];
    for (explicit, from_matrix, label, cell_ref) in overrides {
        if let Some(val) = explicit {
            if val != from_matrix {
                errors.push(format!(
                    "payoff.{label} does not match payoff.matrix{cell_ref}"
                ));
            }
        }
    }
}

fn fallback_payoff(config: PayoffConfig) -> PayoffMatrix {
    let reward = config.r.unwrap_or(DEFAULT_R);
    let sucker = config.s.unwrap_or(DEFAULT_S);
    let temptation = config.t.unwrap_or(DEFAULT_T);
    let punishment = config.p.unwrap_or(DEFAULT_P);
    PayoffMatrix::from_matrix([
        [[reward, reward], [sucker, temptation]],
        [[temptation, sucker], [punishment, punishment]],
    ])
}
