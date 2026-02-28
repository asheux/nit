use crate::game::Action;
use crate::history::{History, RoundRecord};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub trait Strategy: Send {
    fn id(&self) -> &str;
    fn reset(&mut self);
    fn next_action(&mut self, history: &History, player_a: bool) -> Action;
    fn last_halted(&self) -> bool {
        true
    }
    fn tm_stats(&self) -> Option<&TmRunStats> {
        None
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StrategyKind {
    Fsm,
    Ca,
    OneSidedTm,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    OpponentLastAction,
    SelfLastAction,
    JointLastAction,
}

impl Default for InputMode {
    fn default() -> Self {
        InputMode::OpponentLastAction
    }
}

impl InputMode {
    pub fn alphabet_size(self) -> usize {
        match self {
            InputMode::OpponentLastAction | InputMode::SelfLastAction => 2,
            InputMode::JointLastAction => 4,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TmMove {
    #[serde(rename = "L")]
    Left,
    #[serde(rename = "R")]
    Right,
    #[serde(rename = "S")]
    Stay,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct TmTransition {
    pub write: u8,
    #[serde(rename = "move")]
    pub move_dir: TmMove,
    pub next: u16,
}

pub fn decode_tm_rule_code_wolfram(
    rule_code: u64,
    states: usize,
    symbols: usize,
) -> (Vec<TmTransition>, u64) {
    let total = states.saturating_mul(symbols);
    let mut transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Left,
            next: 1,
        };
        total
    ];
    if states == 0 || symbols == 0 {
        return (transitions, rule_code);
    }
    let base = (symbols as u64) * (states as u64) * 2;
    if base == 0 {
        return (transitions, rule_code);
    }
    let mut code = rule_code;
    for state in (1..=states).rev() {
        for read in 0..symbols {
            let digit = code % base;
            code /= base;
            let move_idx = (digit % 2) as u8;
            let write = ((digit / 2) % symbols as u64) as u8;
            let next = (digit / (2 * symbols as u64)) as u16 + 1;
            let move_dir = if move_idx == 0 {
                TmMove::Left
            } else {
                TmMove::Right
            };
            let idx = (state - 1) * symbols + read;
            if let Some(slot) = transitions.get_mut(idx) {
                *slot = TmTransition {
                    write,
                    move_dir,
                    next,
                };
            }
        }
    }
    (transitions, code)
}

pub fn tm_max_index(states: usize, symbols: usize) -> Option<u128> {
    let base = (2u128)
        .checked_mul(states as u128)?
        .checked_mul(symbols as u128)?;
    let exp = states.checked_mul(symbols)? as u32;
    checked_pow_u128(base, exp)?.checked_sub(1)
}

pub fn fsm_count(states: usize, actions: usize) -> Option<u128> {
    let transitions = checked_pow_u128(states as u128, states.checked_mul(actions)? as u32)?;
    let outputs = checked_pow_u128(actions as u128, states as u32)?;
    transitions.checked_mul(outputs)
}

pub fn decode_fsm_notebook_index(
    index: u64,
    states: usize,
    actions: usize,
) -> Result<(Vec<Action>, Vec<Vec<usize>>), String> {
    if states == 0 {
        return Err("fsm decode requires states > 0".to_string());
    }
    if actions == 0 {
        return Err("fsm decode requires actions > 0".to_string());
    }
    let max = fsm_count(states, actions)
        .ok_or_else(|| "fsm index space overflows u128 for this (states, actions)".to_string())?;
    if (index as u128) >= max {
        return Err(format!("fsm index {index} out of range (0..{})", max - 1));
    }

    let n = states.saturating_mul(actions);
    let action_block = checked_pow_u128(actions as u128, states as u32)
        .ok_or_else(|| "fsm action block overflows u128".to_string())?;
    let (transition_code, output_code) =
        floor_div_rem_i128(index as i128 - 1, action_block as i128);

    let next_digits = if states == 1 {
        vec![0usize; n]
    } else {
        integer_digits_signed_abs(transition_code, states, n)
    };
    let output_digits = if actions == 1 {
        vec![0usize; states]
    } else {
        integer_digits_unsigned(output_code as u128, actions, states)
    };

    let outputs = output_digits
        .into_iter()
        .map(|digit| {
            if digit == 0 {
                Action::Cooperate
            } else {
                Action::Defect
            }
        })
        .collect::<Vec<_>>();

    let mut transitions = vec![vec![0usize; actions]; states];
    for state_idx in 0..states {
        for input_idx in 0..actions {
            let flat_idx = state_idx.saturating_mul(actions).saturating_add(input_idx);
            let next = next_digits.get(flat_idx).copied().unwrap_or(0);
            transitions[state_idx][input_idx] = next.min(states - 1);
        }
    }

    Ok((outputs, transitions))
}

pub fn history_to_input_u64(history: &History) -> Option<u64> {
    let mut value: u64 = 0;
    for round in history.iter() {
        let pair = ((action_bit(round.a) as u64) << 1) | action_bit(round.b) as u64;
        value = value.checked_mul(4)?.checked_add(pair)?;
    }
    Some(value)
}

#[derive(Clone, Debug)]
pub struct FsmStrategy {
    id: String,
    start_state: usize,
    state: usize,
    outputs: Vec<Action>,
    transitions: Vec<usize>,
    alphabet: usize,
}

impl FsmStrategy {
    pub fn new(
        id: impl Into<String>,
        start_state: usize,
        outputs: Vec<Action>,
        input_mode: InputMode,
        transitions: Vec<Vec<usize>>,
    ) -> Self {
        let alphabet = if transitions.is_empty() {
            input_mode.alphabet_size().max(2)
        } else {
            transitions[0].len().max(1)
        };
        let mut flat = Vec::new();
        for row in transitions {
            for entry in row {
                flat.push(entry);
            }
        }
        Self {
            id: id.into(),
            start_state,
            state: start_state,
            outputs,
            transitions: flat,
            alphabet,
        }
    }
}

impl Strategy for FsmStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {
        self.state = self.start_state;
    }

    fn next_action(&mut self, history: &History, player_a: bool) -> Action {
        if let Some(last) = history.last() {
            let opponent = if player_a { last.b } else { last.a };
            let symbol = action_bit(opponent) as usize;
            let idx = self
                .state
                .saturating_mul(self.alphabet)
                .saturating_add(symbol);
            if let Some(next_state) = self.transitions.get(idx).copied() {
                self.state = next_state;
            }
        }
        self.outputs
            .get(self.state)
            .copied()
            .unwrap_or(Action::Cooperate)
    }
}

#[derive(Clone, Debug)]
pub struct CaStrategy {
    id: String,
    rule_code: u64,
    symbols: u8,
    two_r: u32,
    steps: u32,
    rule_table: Vec<u8>,
    bit_window: BitWindow,
    last_history_len: usize,
}

#[derive(Clone, Debug)]
pub struct CaRunResult {
    pub rows: Vec<Vec<u8>>,
    pub output_symbol: u8,
    pub steps_executed: u32,
    pub stopped_early: bool,
}

pub fn decode_ca_rule_table(rule_code: u64, symbols: u8, two_r: u32) -> Vec<u8> {
    let neighborhood = two_r.saturating_add(1) as usize;
    let table_len = checked_pow_usize(symbols.max(2) as usize, neighborhood).unwrap_or(0);
    integer_digits_unsigned(rule_code as u128, symbols.max(2) as usize, table_len)
        .into_iter()
        .map(|digit| digit as u8)
        .collect()
}

fn ca_transition_symbol(rule_table: &[u8], symbols: u8, window: &[u8]) -> u8 {
    let base = symbols.max(2) as usize;
    let mut idx = 0usize;
    for &digit in window {
        idx = idx.saturating_mul(base).saturating_add(digit as usize);
    }
    rule_table.get(idx).copied().unwrap_or(0)
}

pub fn run_shrinking_ca(
    rule_table: &[u8],
    symbols: u8,
    two_r: u32,
    steps: u32,
    input_row: &[u8],
) -> CaRunResult {
    let mut row = if input_row.is_empty() {
        vec![0]
    } else {
        input_row.to_vec()
    };
    let mut rows = vec![row.clone()];
    let mut steps_executed = 0u32;
    let mut stopped_early = false;
    let two_r = two_r as usize;
    let neighborhood = two_r.saturating_add(1);

    for _ in 0..steps {
        if neighborhood == 0 || row.len() <= two_r {
            stopped_early = true;
            break;
        }
        let next_len = row.len().saturating_sub(two_r);
        if next_len == 0 {
            stopped_early = true;
            break;
        }
        let mut next = Vec::with_capacity(next_len);
        for start in 0..next_len {
            let end = start.saturating_add(neighborhood);
            let value = ca_transition_symbol(rule_table, symbols, &row[start..end]);
            next.push(value);
        }
        row = next;
        rows.push(row.clone());
        steps_executed = steps_executed.saturating_add(1);
    }

    let output_symbol = row.last().copied().unwrap_or(0);
    CaRunResult {
        rows,
        output_symbol,
        steps_executed,
        stopped_early,
    }
}

impl CaStrategy {
    pub fn new(id: impl Into<String>, rule_code: u64, symbols: u8, two_r: u32, steps: u32) -> Self {
        let rule_table = decode_ca_rule_table(rule_code, symbols, two_r);
        let suffix_len = two_r.saturating_mul(steps).saturating_add(1).max(1) as usize;
        Self {
            id: id.into(),
            rule_code,
            symbols: symbols.max(2),
            two_r,
            steps,
            rule_table,
            bit_window: BitWindow::new(suffix_len),
            last_history_len: 0,
        }
    }

    pub fn rule_code(&self) -> u64 {
        self.rule_code
    }

    fn sync_history(&mut self, history: &History) {
        sync_bit_window(&mut self.bit_window, history, &mut self.last_history_len);
    }

    fn run_shrinking_ca(&self, row: Vec<u8>) -> u8 {
        let run = run_shrinking_ca(&self.rule_table, self.symbols, self.two_r, self.steps, &row);
        run.output_symbol
    }
}

impl Strategy for CaStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {
        self.bit_window.clear();
        self.last_history_len = 0;
    }

    fn next_action(&mut self, history: &History, _player_a: bool) -> Action {
        self.sync_history(history);
        if history.is_empty() {
            return Action::Cooperate;
        }
        let bits = self.bit_window.to_vec();
        let symbol = self.run_shrinking_ca(bits);
        if symbol == 0 {
            Action::Cooperate
        } else {
            Action::Defect
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TmRunStats {
    pub rounds: u64,
    pub steps: u64,
    pub min_steps: u32,
    pub max_steps: u32,
    pub output_events: u64,
    pub fallback: u64,
    pub max_steps_hits: u64,
}

impl TmRunStats {
    pub fn merge(&mut self, other: &TmRunStats) {
        if other.rounds > 0 {
            if self.rounds == 0 {
                self.min_steps = other.min_steps;
                self.max_steps = other.max_steps;
            } else {
                self.min_steps = self.min_steps.min(other.min_steps);
                self.max_steps = self.max_steps.max(other.max_steps);
            }
        }
        self.rounds = self.rounds.saturating_add(other.rounds);
        self.steps = self.steps.saturating_add(other.steps);
        self.output_events = self.output_events.saturating_add(other.output_events);
        self.fallback = self.fallback.saturating_add(other.fallback);
        self.max_steps_hits = self.max_steps_hits.saturating_add(other.max_steps_hits);
    }
}

#[derive(Clone, Debug)]
pub struct TmTraceStep {
    pub step: usize,
    pub state: u16,
    pub head_before: usize,
    pub read: u8,
    pub next: u16,
    pub write: u8,
    pub move_dir: TmMove,
    pub head_after: usize,
    pub tape: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct TmTrace {
    pub input_digits: Vec<u8>,
    pub initial_tape: Vec<u8>,
    pub initial_head: usize,
    pub steps: Vec<TmTraceStep>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TmStopReason {
    Output,
    MaxSteps,
    MissingTransition,
    InvalidState,
}

#[derive(Clone, Debug)]
pub struct TmRunResult {
    pub output_value: Option<u64>,
    pub output_symbol: Option<u8>,
    pub halted: bool,
    pub steps_taken: u32,
    pub stop_reason: TmStopReason,
    pub trace: Option<TmTrace>,
}

pub fn run_one_sided_tm_from_integer(
    transitions: &[TmTransition],
    symbols: u8,
    start_state: u16,
    blank: u8,
    input: u64,
    max_steps: u32,
    with_trace: bool,
) -> TmRunResult {
    let digits = digits_in_base(input, symbols.max(2));
    run_one_sided_tm(
        transitions,
        symbols,
        start_state,
        blank,
        &digits,
        max_steps,
        with_trace,
    )
}

pub fn run_one_sided_tm(
    transitions: &[TmTransition],
    symbols: u8,
    start_state: u16,
    blank: u8,
    input_digits: &[u8],
    max_steps: u32,
    with_trace: bool,
) -> TmRunResult {
    let symbols = symbols.max(1);
    let digits = if input_digits.is_empty() {
        vec![0]
    } else {
        input_digits.to_vec()
    };
    let mut tape = digits.clone();
    let mut head = tape.len().saturating_sub(1);
    let mut state = start_state;

    let mut trace = if with_trace {
        Some(TmTrace {
            input_digits: digits.clone(),
            initial_tape: tape.clone(),
            initial_head: head,
            steps: Vec::with_capacity(max_steps.min(10_000) as usize),
        })
    } else {
        None
    };

    if max_steps == 0 {
        return TmRunResult {
            output_value: None,
            output_symbol: None,
            halted: false,
            steps_taken: 0,
            stop_reason: TmStopReason::MaxSteps,
            trace,
        };
    }
    if state == 0 {
        return TmRunResult {
            output_value: None,
            output_symbol: None,
            halted: false,
            steps_taken: 0,
            stop_reason: TmStopReason::InvalidState,
            trace,
        };
    }

    for step in 0..(max_steps as usize) {
        let head_before = head;
        let read = tape.get(head_before).copied().unwrap_or(blank);
        let idx = (state.saturating_sub(1) as usize)
            .saturating_mul(symbols as usize)
            .saturating_add(read as usize);
        let Some(trans) = transitions.get(idx).copied() else {
            return TmRunResult {
                output_value: None,
                output_symbol: None,
                halted: false,
                steps_taken: (step + 1) as u32,
                stop_reason: TmStopReason::MissingTransition,
                trace,
            };
        };

        if let Some(cell) = tape.get_mut(head_before) {
            *cell = trans.write;
        }

        if matches!(trans.move_dir, TmMove::Right) && head_before + 1 == tape.len() {
            let output_value = digits_to_u64(&tape, symbols);
            let output_symbol = output_value.map(|value| (value % symbols as u64) as u8);
            if let Some(trace) = trace.as_mut() {
                trace.steps.push(TmTraceStep {
                    step: step + 1,
                    state,
                    head_before,
                    read,
                    next: trans.next,
                    write: trans.write,
                    move_dir: trans.move_dir,
                    head_after: head_before + 1,
                    tape: tape.clone(),
                });
            }
            return TmRunResult {
                output_value,
                output_symbol,
                halted: true,
                steps_taken: (step + 1) as u32,
                stop_reason: TmStopReason::Output,
                trace,
            };
        }

        let mut head_after = head_before;
        match trans.move_dir {
            TmMove::Left => {
                if head_after > 0 {
                    head_after -= 1;
                }
            }
            TmMove::Stay => {}
            TmMove::Right => {
                if head_after + 1 < tape.len() {
                    head_after += 1;
                }
            }
        }

        if let Some(trace) = trace.as_mut() {
            trace.steps.push(TmTraceStep {
                step: step + 1,
                state,
                head_before,
                read,
                next: trans.next,
                write: trans.write,
                move_dir: trans.move_dir,
                head_after,
                tape: tape.clone(),
            });
        }

        head = head_after;
        state = trans.next;
        if state == 0 {
            return TmRunResult {
                output_value: None,
                output_symbol: None,
                halted: false,
                steps_taken: (step + 1) as u32,
                stop_reason: TmStopReason::InvalidState,
                trace,
            };
        }
    }

    TmRunResult {
        output_value: None,
        output_symbol: None,
        halted: false,
        steps_taken: max_steps,
        stop_reason: TmStopReason::MaxSteps,
        trace,
    }
}

#[derive(Clone, Debug)]
pub struct OneSidedTmStrategy {
    id: String,
    symbols: u8,
    start_state: u16,
    blank: u8,
    max_steps_per_round: u32,
    transitions: Vec<TmTransition>,
    input_suffix: InputSuffix,
    last_halted: bool,
    stats: TmRunStats,
}

impl OneSidedTmStrategy {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        symbols: u8,
        start_state: u16,
        blank: u8,
        _fallback_symbol: u8,
        max_steps_per_round: u32,
        _input_mode: InputMode,
        _output_map: Vec<Action>,
        transitions: Vec<TmTransition>,
    ) -> Self {
        let symbols = symbols.max(2);
        let width = max_steps_per_round as usize + 1;
        Self {
            id: id.into(),
            symbols,
            start_state,
            blank,
            max_steps_per_round,
            transitions,
            input_suffix: InputSuffix::new(symbols, width.max(1)),
            last_halted: true,
            stats: TmRunStats::default(),
        }
    }

    pub fn stats(&self) -> &TmRunStats {
        &self.stats
    }

    fn action_from_symbol(&self, symbol: u8) -> Action {
        let base = self.symbols.max(1);
        if symbol % base == 0 {
            Action::Cooperate
        } else {
            Action::Defect
        }
    }

    fn action_from_output_value(&self, output: u64) -> Action {
        let symbol = (output % self.symbols.max(1) as u64) as u8;
        self.action_from_symbol(symbol)
    }

    fn sync_input(&mut self, history: &History) {
        let len = history.len();
        if len < self.input_suffix.history_len || len > self.input_suffix.history_len + 1 {
            self.input_suffix.reset();
            for round in history.iter() {
                self.input_suffix.push_round(round);
            }
            self.input_suffix.history_len = len;
            return;
        }
        if len == self.input_suffix.history_len + 1 {
            if let Some(last) = history.last() {
                self.input_suffix.push_round(last);
            }
            self.input_suffix.history_len = len;
        }
    }
}

impl Strategy for OneSidedTmStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {
        self.input_suffix.reset();
        self.last_halted = true;
        self.stats = TmRunStats::default();
    }

    fn next_action(&mut self, history: &History, _player_a: bool) -> Action {
        self.sync_input(history);
        let input_digits = self.input_suffix.msd_digits();
        let run = run_one_sided_tm(
            &self.transitions,
            self.symbols,
            self.start_state,
            self.blank,
            &input_digits,
            self.max_steps_per_round,
            false,
        );

        self.last_halted = run.halted;
        self.stats.rounds = self.stats.rounds.saturating_add(1);
        self.stats.steps = self.stats.steps.saturating_add(run.steps_taken as u64);
        if self.stats.rounds == 1 {
            self.stats.min_steps = run.steps_taken;
            self.stats.max_steps = run.steps_taken;
        } else {
            self.stats.min_steps = self.stats.min_steps.min(run.steps_taken);
            self.stats.max_steps = self.stats.max_steps.max(run.steps_taken);
        }
        if run.halted {
            self.stats.output_events = self.stats.output_events.saturating_add(1);
        } else {
            self.stats.fallback = self.stats.fallback.saturating_add(1);
            if matches!(run.stop_reason, TmStopReason::MaxSteps) {
                self.stats.max_steps_hits = self.stats.max_steps_hits.saturating_add(1);
            }
        }

        if let Some(output) = run.output_value {
            self.action_from_output_value(output)
        } else {
            Action::Defect
        }
    }

    fn last_halted(&self) -> bool {
        self.last_halted
    }

    fn tm_stats(&self) -> Option<&TmRunStats> {
        Some(&self.stats)
    }
}

#[derive(Clone, Debug)]
struct BitWindow {
    max_len: usize,
    bits: VecDeque<u8>,
}

impl BitWindow {
    fn new(max_len: usize) -> Self {
        Self {
            max_len: max_len.max(1),
            bits: VecDeque::new(),
        }
    }

    fn clear(&mut self) {
        self.bits.clear();
    }

    fn push_round(&mut self, record: RoundRecord) {
        self.push_bit(action_bit(record.a));
        self.push_bit(action_bit(record.b));
    }

    fn push_bit(&mut self, bit: u8) {
        self.bits.push_back(bit.min(1));
        while self.bits.len() > self.max_len {
            self.bits.pop_front();
        }
    }

    fn to_vec(&self) -> Vec<u8> {
        self.bits.iter().copied().collect()
    }
}

fn sync_bit_window(window: &mut BitWindow, history: &History, last_history_len: &mut usize) {
    let len = history.len();
    if len < *last_history_len || len > (*last_history_len).saturating_add(1) {
        window.clear();
        for record in history.iter() {
            window.push_round(record);
        }
        *last_history_len = len;
        return;
    }
    if len == *last_history_len + 1 {
        if let Some(last) = history.last() {
            window.push_round(last);
        }
        *last_history_len = len;
    }
}

#[derive(Clone, Debug)]
struct InputSuffix {
    base: u8,
    width: usize,
    digits_le: Vec<u8>,
    prefix_nonzero: bool,
    history_len: usize,
}

impl InputSuffix {
    fn new(base: u8, width: usize) -> Self {
        Self {
            base: base.max(2),
            width: width.max(1),
            digits_le: vec![0],
            prefix_nonzero: false,
            history_len: 0,
        }
    }

    fn reset(&mut self) {
        self.digits_le.clear();
        self.digits_le.push(0);
        self.prefix_nonzero = false;
        self.history_len = 0;
    }

    fn push_round(&mut self, round: RoundRecord) {
        let pair = ((action_bit(round.a) << 1) | action_bit(round.b)) as u16;
        self.mul_add(4, pair);
    }

    fn mul_add(&mut self, mul: u16, add: u16) {
        let base = self.base as u16;
        let mut carry = add;
        for digit in &mut self.digits_le {
            let value = (*digit as u16).saturating_mul(mul).saturating_add(carry);
            *digit = (value % base) as u8;
            carry = value / base;
        }
        while carry > 0 {
            if self.digits_le.len() < self.width {
                self.digits_le.push((carry % base) as u8);
                carry /= base;
            } else {
                self.prefix_nonzero = true;
                break;
            }
        }
        while self.digits_le.len() > self.width {
            let popped = self.digits_le.pop();
            if popped.unwrap_or(0) != 0 {
                self.prefix_nonzero = true;
            }
        }
        if self.prefix_nonzero {
            self.trim_most_significant_zeros_with_prefix();
        } else {
            self.trim_redundant_high_zeros();
        }
    }

    fn msd_digits(&self) -> Vec<u8> {
        if self.digits_le.is_empty() {
            return vec![0];
        }
        let mut out = self.digits_le.iter().rev().copied().collect::<Vec<_>>();
        if !self.prefix_nonzero {
            while out.len() > 1 && out.first() == Some(&0) {
                out.remove(0);
            }
        }
        if out.is_empty() {
            vec![0]
        } else {
            out
        }
    }

    fn trim_redundant_high_zeros(&mut self) {
        while self.digits_le.len() > 1 && self.digits_le.last() == Some(&0) {
            self.digits_le.pop();
        }
    }

    fn trim_most_significant_zeros_with_prefix(&mut self) {
        while self.digits_le.len() > self.width {
            self.digits_le.pop();
        }
        while self.digits_le.len() > 1 && self.digits_le.last() == Some(&0) {
            if self.digits_le.len() == self.width {
                break;
            }
            self.digits_le.pop();
        }
    }
}

fn action_bit(action: Action) -> u8 {
    match action {
        Action::Cooperate => 0,
        Action::Defect => 1,
    }
}

fn digits_in_base(input: u64, base: u8) -> Vec<u8> {
    let base = base.max(2) as u64;
    if input == 0 {
        return vec![0];
    }
    let mut value = input;
    let mut digits = Vec::new();
    while value > 0 {
        digits.push((value % base) as u8);
        value /= base;
    }
    digits.reverse();
    digits
}

fn digits_to_u64(digits: &[u8], base: u8) -> Option<u64> {
    let base_u64 = base.max(2) as u64;
    let mut value = 0u64;
    for &digit in digits {
        value = value.checked_mul(base_u64)?;
        value = value.checked_add(digit as u64)?;
    }
    Some(value)
}

fn floor_div_rem_i128(numer: i128, denom: i128) -> (i128, i128) {
    let mut q = numer / denom;
    let mut r = numer % denom;
    if r < 0 {
        q -= 1;
        r += denom;
    }
    (q, r)
}

fn integer_digits_signed_abs(value: i128, base: usize, len: usize) -> Vec<usize> {
    integer_digits_unsigned(value.unsigned_abs(), base, len)
}

fn integer_digits_unsigned(mut value: u128, base: usize, len: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let base_u128 = base.max(2) as u128;
    let mut digits = vec![0usize; len];
    for idx in (0..len).rev() {
        digits[idx] = (value % base_u128) as usize;
        value /= base_u128;
    }
    digits
}

fn checked_pow_u128(base: u128, exp: u32) -> Option<u128> {
    let mut value = 1u128;
    for _ in 0..exp {
        value = value.checked_mul(base)?;
    }
    Some(value)
}

fn checked_pow_usize(base: usize, exp: usize) -> Option<usize> {
    let mut value = 1usize;
    for _ in 0..exp {
        value = value.checked_mul(base)?;
    }
    Some(value)
}
