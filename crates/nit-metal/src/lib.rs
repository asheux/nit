#[derive(Clone, Debug)]
pub struct MatchPair {
    pub a_idx: u32,
    pub b_idx: u32,
}

pub const CA_MAX_WINDOW: u32 = 1024;
pub const TM_MAX_WIDTH: u32 = 256;

#[derive(Clone, Debug)]
pub struct ScorePair {
    pub a_total: i64,
    pub b_total: i64,
}

#[derive(Clone, Debug)]
pub struct EvalCommon {
    pub rounds: u32,
    pub payoff: [[[i32; 2]; 2]; 2],
    pub timeout_lose: i32,
    pub timeout_win: i32,
    pub pairs: Vec<MatchPair>,
}

#[derive(Clone, Debug)]
pub struct FsmBatch {
    pub states: u32,
    pub alphabet: u32,
    pub starts: Vec<u32>,
    pub outputs: Vec<u32>,
    pub transitions: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct CaBatch {
    pub symbols: u32,
    pub two_r: u32,
    pub steps: u32,
    pub rule_table_len: u32,
    pub rule_tables: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct TmTransitionPacked {
    pub write: u32,
    pub move_dir: u32,
    pub next: u32,
}

#[derive(Clone, Debug)]
pub struct TmBatch {
    pub states: u32,
    pub symbols: u32,
    pub blank: u32,
    pub max_steps: u32,
    pub start_states: Vec<u32>,
    pub transitions: Vec<TmTransitionPacked>,
}

#[derive(Clone, Debug)]
pub enum BatchPayload {
    Fsm(FsmBatch),
    Ca(CaBatch),
    Tm(TmBatch),
}

#[derive(Clone, Debug)]
pub struct BatchRequest {
    pub common: EvalCommon,
    pub payload: BatchPayload,
}

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::try_evaluate_batch;

#[cfg(not(target_os = "macos"))]
pub fn try_evaluate_batch(_request: &BatchRequest) -> Result<Option<Vec<ScorePair>>, String> {
    Ok(None)
}
