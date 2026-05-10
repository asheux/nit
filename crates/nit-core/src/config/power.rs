/// Power-management settings. Controls macOS idle-sleep behaviour while a
/// nit session is running. The lid is out of scope — clamshell sleep is
/// enforced by the OS and can't be blocked by user-space assertions.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PowerConfig {
    /// Hold a `caffeinate -i` while one or more agent turns are in flight.
    /// Prevents the system from hibernating on inactivity mid-swarm, which
    /// otherwise SIGSTOPs the Claude / Codex subprocesses and kills their
    /// API connections. Display sleep is unaffected. Defaults to `true` —
    /// long-running swarms are nit's headline use case and a stuck mission
    /// caused by idle-sleep is far more annoying than the marginal battery
    /// cost. Set to `false` to opt out.
    #[serde(default = "super::default_true")]
    pub prevent_idle_sleep_during_turns: bool,
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            prevent_idle_sleep_during_turns: super::default_true(),
        }
    }
}
