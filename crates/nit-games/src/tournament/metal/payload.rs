use crate::config::{NormalizedConfig, StrategySpec, StrategySpecKind};
use crate::game::Action;
use crate::strategy::TmMove;
use nit_metal::{BatchEvalConfig, BatchPayload, CaBatch, FsmBatch, TmBatch};

pub(super) fn ensure_uniform<T: PartialEq>(slot: &mut Option<T>, incoming: T) -> Option<()> {
    match slot {
        Some(existing) if *existing != incoming => None,
        slot_ref @ None => {
            *slot_ref = Some(incoming);
            Some(())
        }
        _ => Some(()),
    }
}

pub(super) fn move_dir_code(direction: TmMove) -> u32 {
    match direction {
        TmMove::Left => 0,
        TmMove::Right => 1,
        TmMove::Stay => 2,
    }
}

pub(super) fn metal_batch_eval_config(config: &NormalizedConfig) -> BatchEvalConfig {
    let (timeout_lose, timeout_win) = config.payoff.min_max();
    BatchEvalConfig {
        rounds: config.rounds,
        payoff: config.payoff.matrix,
        timeout_lose,
        timeout_win,
    }
}

pub(super) fn build_metal_batch_payload(strategies: &[StrategySpec]) -> Option<BatchPayload> {
    let first = strategies.first()?;
    match &first.kind {
        StrategySpecKind::Fsm { .. } => build_metal_fsm_payload(strategies).map(BatchPayload::Fsm),
        StrategySpecKind::Ca { .. } => build_metal_ca_payload(strategies).map(BatchPayload::Ca),
        StrategySpecKind::OneSidedTm { .. } => {
            build_metal_tm_payload(strategies).map(BatchPayload::Tm)
        }
    }
}

struct ValidatedFsm<'a> {
    state_count: usize,
    alphabet_size: usize,
    start_state: usize,
    outputs: &'a [Action],
    transition_table: &'a [Vec<usize>],
}

fn validate_fsm_spec(spec: &StrategySpec) -> Option<ValidatedFsm<'_>> {
    let StrategySpecKind::Fsm {
        num_states,
        start_state,
        outputs: state_outputs,
        input_mode,
        transitions: transition_table,
        ..
    } = &spec.kind
    else {
        return None;
    };

    let effective_mode = input_mode.unwrap_or(crate::strategy::InputMode::OpponentLastAction);
    if !matches!(
        effective_mode,
        crate::strategy::InputMode::OpponentLastAction
    ) {
        return None;
    }

    let state_count = (*num_states).max(state_outputs.len());
    let alphabet_size = transition_table.first().map_or(0, Vec::len);

    if alphabet_size != 2
        || state_count == 0
        || transition_table.len() != state_count
        || *start_state >= state_count
    {
        return None;
    }

    if transition_table
        .iter()
        .any(|row| row.len() != alphabet_size)
    {
        return None;
    }

    Some(ValidatedFsm {
        state_count,
        alphabet_size,
        start_state: *start_state,
        outputs: state_outputs,
        transition_table,
    })
}

fn build_metal_fsm_payload(strategies: &[StrategySpec]) -> Option<FsmBatch> {
    let mut uniform_states: Option<usize> = None;
    let mut uniform_alphabet: Option<usize> = None;
    let mut start_indices = Vec::with_capacity(strategies.len());
    let mut flat_outputs = Vec::new();
    let mut flat_transitions = Vec::new();

    for spec in strategies {
        let fsm = validate_fsm_spec(spec)?;
        let ValidatedFsm {
            state_count,
            alphabet_size,
            start_state: start,
            outputs,
            transition_table: table,
        } = fsm;

        ensure_uniform(&mut uniform_states, state_count)?;
        ensure_uniform(&mut uniform_alphabet, alphabet_size)?;

        start_indices.push(start as u32);

        flat_outputs.extend(outputs.iter().map(|action| match action {
            Action::Cooperate => 0u32,
            Action::Defect => 1u32,
        }));
        let padding_needed = state_count.saturating_sub(outputs.len());
        flat_outputs.extend(std::iter::repeat_n(0u32, padding_needed));

        let any_out_of_range = table.iter().flatten().any(|&n| n >= state_count);
        if any_out_of_range {
            return None;
        }
        flat_transitions.extend(table.iter().flatten().map(|&n| n as u32));
    }

    Some(FsmBatch {
        states: uniform_states? as u32,
        alphabet: uniform_alphabet? as u32,
        starts: start_indices,
        outputs: flat_outputs,
        transitions: flat_transitions,
    })
}

fn build_metal_ca_payload(strategies: &[StrategySpec]) -> Option<CaBatch> {
    let mut uniform_symbols: Option<u32> = None;
    let mut uniform_two_r: Option<u32> = None;
    let mut uniform_steps: Option<u32> = None;
    let mut uniform_table_len: Option<u32> = None;
    let mut flat_rule_tables = Vec::new();

    for spec in strategies {
        let StrategySpecKind::Ca { n, k, r, t } = &spec.kind else {
            return None;
        };

        let derived_two_r = (*r * 2.0).round() as u32;
        if ((*r * 2.0) - derived_two_r as f32).abs() > 0.0001 {
            return None;
        }

        ensure_uniform(&mut uniform_symbols, *k as u32)?;
        ensure_uniform(&mut uniform_two_r, derived_two_r)?;
        ensure_uniform(&mut uniform_steps, *t)?;

        let decoded_table = crate::strategy::decode_ca_rule_table(*n, *k, derived_two_r);
        ensure_uniform(&mut uniform_table_len, decoded_table.len() as u32)?;

        flat_rule_tables.extend(decoded_table.into_iter().map(u32::from));
    }

    Some(CaBatch {
        symbols: uniform_symbols?,
        two_r: uniform_two_r?,
        steps: uniform_steps?,
        rule_table_len: uniform_table_len?,
        rule_tables: flat_rule_tables,
    })
}

fn build_metal_tm_payload(strategies: &[StrategySpec]) -> Option<TmBatch> {
    let mut uniform_states: Option<u32> = None;
    let mut uniform_symbols: Option<u32> = None;
    let mut uniform_blank: Option<u32> = None;
    let mut uniform_max_steps: Option<u32> = None;
    let mut start_states = Vec::with_capacity(strategies.len());
    let mut packed_transitions = Vec::new();

    for spec in strategies {
        let StrategySpecKind::OneSidedTm {
            states: tm_state_count,
            symbols: tm_symbol_count,
            start_state,
            blank: tm_blank_symbol,
            max_steps_per_round,
            transitions: tm_rules,
            ..
        } = &spec.kind
        else {
            return None;
        };

        ensure_uniform(&mut uniform_states, *tm_state_count as u32)?;
        ensure_uniform(&mut uniform_symbols, *tm_symbol_count as u32)?;
        ensure_uniform(&mut uniform_blank, *tm_blank_symbol as u32)?;
        ensure_uniform(&mut uniform_max_steps, *max_steps_per_round)?;

        let expected_entries = (*tm_state_count as usize).saturating_mul(*tm_symbol_count as usize);
        if tm_rules.len() != expected_entries {
            return None;
        }

        let bounds_violated = *start_state > *tm_state_count
            || tm_rules
                .iter()
                .any(|rule| rule.write >= *tm_symbol_count || rule.next > *tm_state_count);
        if bounds_violated {
            return None;
        }

        start_states.push(*start_state as u32);
        packed_transitions.extend(tm_rules.iter().map(|rule| nit_metal::TmTransitionPacked {
            write: u32::from(rule.write),
            move_dir: move_dir_code(rule.move_dir),
            next: u32::from(rule.next),
        }));
    }

    Some(TmBatch {
        states: uniform_states?,
        symbols: uniform_symbols?,
        blank: uniform_blank?,
        max_steps: uniform_max_steps?,
        start_states,
        transitions: packed_transitions,
    })
}
