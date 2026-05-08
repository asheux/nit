use crate::strategy::{TmMove, TmTransition};

pub(super) fn parse_tm_table_transitions(
    raw: &toml::Value,
    states: usize,
    symbols: usize,
) -> Result<Vec<TmTransition>, String> {
    let rows = raw
        .as_array()
        .ok_or_else(|| "expected transitions to be an array".to_string())?;
    if rows.len() != states {
        return Err(format!(
            "transitions table must have {states} rows (one per state)"
        ));
    }
    let total = states.saturating_mul(symbols);
    let mut transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Stay,
            next: 0,
        };
        total
    ];
    for (state_idx, row_val) in rows.iter().enumerate() {
        let row = row_val
            .as_array()
            .ok_or_else(|| format!("transitions[{state_idx}] must be an array"))?;
        if row.len() != symbols {
            return Err(format!(
                "transitions[{state_idx}] must have {symbols} entries (one per symbol)"
            ));
        }
        for (read_idx, entry_val) in row.iter().enumerate() {
            let idx = state_idx * symbols + read_idx;
            transitions[idx] =
                parse_tm_table_cell(entry_val, state_idx, read_idx, states, symbols)?;
        }
    }
    Ok(transitions)
}

fn parse_tm_table_cell(
    entry_val: &toml::Value,
    state_idx: usize,
    read_idx: usize,
    states: usize,
    symbols: usize,
) -> Result<TmTransition, String> {
    let entry = entry_val.as_array().ok_or_else(|| {
        format!("transitions[{state_idx}][{read_idx}] must be [next, write, move]")
    })?;
    if entry.len() != 3 {
        return Err(format!(
            "transitions[{state_idx}][{read_idx}] must be [next, write, move]"
        ));
    }
    let next = entry[0]
        .as_integer()
        .ok_or_else(|| format!("transitions[{state_idx}][{read_idx}][0] must be an integer"))?;
    let write = entry[1]
        .as_integer()
        .ok_or_else(|| format!("transitions[{state_idx}][{read_idx}][1] must be an integer"))?;
    let move_dir = parse_tm_move_value(&entry[2], state_idx, read_idx)?;
    if next < 0 || next as usize > states {
        return Err(format!(
            "transitions[{state_idx}][{read_idx}][0] next {next} out of range (0..={states})"
        ));
    }
    if write < 0 || write as usize >= symbols {
        return Err(format!(
            "transitions[{state_idx}][{read_idx}][1] write {write} out of range (symbols={symbols})"
        ));
    }
    Ok(TmTransition {
        write: write as u8,
        move_dir,
        next: next as u16,
    })
}

fn parse_tm_move_value(
    value: &toml::Value,
    state_idx: usize,
    read_idx: usize,
) -> Result<TmMove, String> {
    if let Some(move_int) = value.as_integer() {
        return match move_int {
            -1 => Ok(TmMove::Left),
            0 => Ok(TmMove::Stay),
            1 => Ok(TmMove::Right),
            other => Err(format!(
                "transitions[{state_idx}][{read_idx}][2] invalid move {other} (expected -1, 0, or 1)"
            )),
        };
    }
    if let Some(move_str) = value.as_str() {
        let normalized = move_str.trim().to_ascii_lowercase();
        return match normalized.as_str() {
            "l" | "left" => Ok(TmMove::Left),
            "r" | "right" => Ok(TmMove::Right),
            "s" | "stay" => Ok(TmMove::Stay),
            _ => Err(format!(
                "transitions[{state_idx}][{read_idx}][2] invalid move '{normalized}'"
            )),
        };
    }
    Err(format!(
        "transitions[{state_idx}][{read_idx}][2] must be a move string or integer"
    ))
}
