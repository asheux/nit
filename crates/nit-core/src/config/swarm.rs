#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SwarmConfig {
    /// Maximum automatic retry rounds when a swarm's verify gate fails
    /// (tests, clippy, fmt, or genome). Each round dispatches a fix task to
    /// the integrator and re-runs the verifier. `0` disables retries.
    #[serde(default = "default_gate_retry_limit")]
    pub gate_retry_limit: u8,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            gate_retry_limit: default_gate_retry_limit(),
        }
    }
}

fn default_gate_retry_limit() -> u8 {
    3
}
