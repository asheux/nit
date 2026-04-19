use std::collections::VecDeque;
use std::time::{Duration, Instant};

const DEFAULT_ECG_CAPACITY: usize = 32;
const ERROR_WINDOW: Duration = Duration::from_secs(60);
const FATAL_LATCH: Duration = Duration::from_secs(60);
const JITTER_THRESHOLD: Duration = Duration::from_millis(140);
const TARGET_RATE_HZ: f64 = 12.0;
const BASE_MAX: f64 = 0.18;
const RATE_EVENT_CAP: u32 = 4;
const EMA_ALPHA: f64 = 0.22;
const SPIKE_DECAY: f64 = 0.72;
const SPIKE_BOOST: f64 = 0.9;
const SPIKE_SUSTAIN_BOOST: f64 = 0.6;
const SPIKE_SUSTAIN_EVERY_TICKS: u32 = 6;
const HEARTBEAT_MIN_INTERVAL: Duration = Duration::from_millis(350);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LabCriticality {
    Idle,
    Ok,
    Warn,
    Hot,
    Crit,
}

impl LabCriticality {
    pub fn label(self) -> &'static str {
        match self {
            LabCriticality::Idle => "IDLE",
            LabCriticality::Ok => "OK",
            LabCriticality::Warn => "WARN",
            LabCriticality::Hot => "HOT",
            LabCriticality::Crit => "CRIT",
        }
    }

    pub fn classify(input: CriticalityInput) -> Self {
        let hb_age = input.hb_age.unwrap_or(Duration::MAX);
        let ag_age = input.ag_age.unwrap_or(Duration::MAX);

        if input.fatal_error {
            return LabCriticality::Crit;
        }
        if input.agent_enabled && input.agent_active_tasks && !input.agent_connected {
            return LabCriticality::Crit;
        }
        if input.job_running && hb_age >= Duration::from_secs(10) {
            return LabCriticality::Crit;
        }
        if input.job_running && hb_age >= Duration::from_secs(5) {
            return LabCriticality::Hot;
        }
        if input.recent_errors >= 3 {
            return LabCriticality::Hot;
        }
        if (input.job_running && hb_age >= Duration::from_secs(2))
            || input.recent_errors > 0
            || (input.agent_enabled && !input.agent_connected && !input.agent_active_tasks)
        {
            return LabCriticality::Warn;
        }
        if (input.job_running && hb_age < Duration::from_secs(2) && input.recent_errors == 0)
            || (!input.job_running
                && input.agent_connected
                && ag_age < Duration::from_secs(5)
                && input.recent_errors == 0)
        {
            return LabCriticality::Ok;
        }
        if !input.job_running && (!input.agent_enabled || !input.agent_active_tasks) {
            return LabCriticality::Idle;
        }
        LabCriticality::Warn
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CriticalityInput {
    pub job_running: bool,
    pub hb_age: Option<Duration>,
    pub agent_enabled: bool,
    pub agent_connected: bool,
    pub agent_active_tasks: bool,
    pub ag_age: Option<Duration>,
    pub recent_errors: u32,
    pub fatal_error: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DiagSeverity {
    Warn,
    Error,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct VitalsCounters {
    pub job_events: u32,
    pub agent_events: u32,
    pub diag_events: u32,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentVitalsState {
    pub enabled: bool,
    pub connected: bool,
    pub active_tasks: bool,
}

impl AgentVitalsState {
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            connected: false,
            active_tasks: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct HeartbeatTracker {
    last_event: Option<Instant>,
}

impl HeartbeatTracker {
    pub fn record(&mut self, now: Instant) {
        if self
            .last_event
            .is_some_and(|last| now.saturating_duration_since(last) < HEARTBEAT_MIN_INTERVAL)
        {
            return;
        }
        self.last_event = Some(now);
    }

    pub fn age(&self, now: Instant) -> Option<Duration> {
        self.last_event.map(|ts| now.saturating_duration_since(ts))
    }
}

#[derive(Clone, Debug)]
struct SlidingWindowCounter {
    window: Duration,
    timestamps: VecDeque<Instant>,
}

impl SlidingWindowCounter {
    fn new(window: Duration) -> Self {
        Self {
            window,
            timestamps: VecDeque::new(),
        }
    }

    fn add(&mut self, now: Instant) {
        self.timestamps.push_back(now);
        self.prune(now);
    }

    fn count(&mut self, now: Instant) -> u32 {
        self.prune(now);
        self.timestamps.len() as u32
    }

    fn prune(&mut self, now: Instant) {
        while let Some(&ts) = self.timestamps.front() {
            if now.saturating_duration_since(ts) > self.window {
                self.timestamps.pop_front();
            } else {
                break;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct LabVitalsSnapshot {
    pub criticality: LabCriticality,
    pub hb_age: Option<Duration>,
    pub ag_age: Option<Duration>,
    pub job_running: bool,
    pub agent_enabled: bool,
    pub agent_connected: bool,
    pub ecg_samples: Vec<u64>,
}

impl LabVitalsSnapshot {
    pub fn waveform(&self, width: usize) -> String {
        sparkline_from_samples(&self.ecg_samples, width)
    }

    pub fn severity_scaled_waveform(&self, width: usize) -> String {
        let scaled = severity_scaled_samples(&self.ecg_samples, self.criticality);
        sparkline_from_samples(&scaled, width)
    }
}

#[derive(Clone, Debug)]
pub struct VitalsState {
    pub job_hb: HeartbeatTracker,
    pub agent_hb: HeartbeatTracker,
    pub rate_ema_hz: f64,
    pub spike: f64,
    ecg_samples: VecDeque<u64>,
    counters: VitalsCounters,
    recent_error_window: SlidingWindowCounter,
    last_job_running: bool,
    last_agent_state: AgentVitalsState,
    last_tick_had_events: bool,
    sustained_event_ticks: u32,
    fatal_until: Option<Instant>,
}

impl Default for VitalsState {
    fn default() -> Self {
        Self::new(DEFAULT_ECG_CAPACITY)
    }
}

impl VitalsState {
    pub fn new(ecg_capacity: usize) -> Self {
        let capacity = ecg_capacity.max(8);
        let mut ecg_samples = VecDeque::with_capacity(capacity);
        for _ in 0..capacity {
            ecg_samples.push_back(0);
        }
        Self {
            job_hb: HeartbeatTracker::default(),
            agent_hb: HeartbeatTracker::default(),
            rate_ema_hz: 0.0,
            spike: 0.0,
            ecg_samples,
            counters: VitalsCounters::default(),
            recent_error_window: SlidingWindowCounter::new(ERROR_WINDOW),
            last_job_running: false,
            last_agent_state: AgentVitalsState::disabled(),
            last_tick_had_events: false,
            sustained_event_ticks: 0,
            fatal_until: None,
        }
    }

    pub fn record_job_event(&mut self, now: Instant) {
        self.counters.job_events = self.counters.job_events.saturating_add(1);
        self.job_hb.record(now);
    }

    pub fn record_agent_event(&mut self, now: Instant) {
        self.counters.agent_events = self.counters.agent_events.saturating_add(1);
        self.agent_hb.record(now);
    }

    pub fn record_diag_event(&mut self, now: Instant, severity: DiagSeverity) {
        self.counters.diag_events = self.counters.diag_events.saturating_add(1);
        if matches!(severity, DiagSeverity::Error) {
            self.recent_error_window.add(now);
        }
    }

    pub fn mark_fatal(&mut self, now: Instant) {
        self.fatal_until = Some(now + FATAL_LATCH);
    }

    pub fn tick(
        &mut self,
        now: Instant,
        dt: Duration,
        job_running: bool,
        agent: AgentVitalsState,
    ) -> LabVitalsSnapshot {
        if self.last_job_running != job_running {
            self.record_job_event(now);
            self.last_job_running = job_running;
        }
        if self.last_agent_state != agent {
            self.record_agent_event(now);
            self.last_agent_state = agent;
        }
        if dt >= JITTER_THRESHOLD {
            self.counters.diag_events = self.counters.diag_events.saturating_add(1);
        }
        if self.fatal_until.is_some_and(|deadline| now >= deadline) {
            self.fatal_until = None;
        }

        let events_this_tick =
            self.counters.job_events + self.counters.agent_events + self.counters.diag_events;
        let dt_secs = dt.as_secs_f64().max(0.001);
        let rate_events = events_this_tick.min(RATE_EVENT_CAP);
        let inst_rate_hz = rate_events as f64 / dt_secs;
        self.rate_ema_hz = (self.rate_ema_hz * (1.0 - EMA_ALPHA)) + (inst_rate_hz * EMA_ALPHA);

        self.spike *= SPIKE_DECAY;
        if events_this_tick > 0 {
            if self.last_tick_had_events {
                self.sustained_event_ticks = self.sustained_event_ticks.saturating_add(1);
                if self
                    .sustained_event_ticks
                    .is_multiple_of(SPIKE_SUSTAIN_EVERY_TICKS)
                {
                    self.spike = (self.spike + SPIKE_SUSTAIN_BOOST).min(1.0);
                }
            } else {
                self.sustained_event_ticks = 1;
                self.spike = (self.spike + SPIKE_BOOST).min(1.0);
            }
        } else {
            self.sustained_event_ticks = 0;
        }
        self.last_tick_had_events = events_this_tick > 0;
        let base = (self.rate_ema_hz / TARGET_RATE_HZ).clamp(0.0, BASE_MAX);
        let amplitude = (base + self.spike).clamp(0.0, 1.0);
        let sample = (amplitude * 100.0).round() as u64;
        self.push_sample(sample);

        let hb_age = self.job_hb.age(now);
        let ag_age = self.agent_hb.age(now);
        let recent_errors = self.recent_error_window.count(now);
        let criticality = LabCriticality::classify(CriticalityInput {
            job_running,
            hb_age,
            agent_enabled: agent.enabled,
            agent_connected: agent.connected,
            agent_active_tasks: agent.active_tasks,
            ag_age,
            recent_errors,
            fatal_error: self.fatal_until.is_some(),
        });

        self.counters = VitalsCounters::default();

        LabVitalsSnapshot {
            criticality,
            hb_age,
            ag_age,
            job_running,
            agent_enabled: agent.enabled,
            agent_connected: agent.connected,
            ecg_samples: self.ecg_samples.iter().copied().collect(),
        }
    }

    fn push_sample(&mut self, sample: u64) {
        if self.ecg_samples.len() == self.ecg_samples.capacity() {
            self.ecg_samples.pop_front();
        }
        self.ecg_samples.push_back(sample.min(100));
    }
}

pub(crate) fn severity_scaled_samples(samples: &[u64], level: LabCriticality) -> Vec<u64> {
    let (floor, gain) = match level {
        LabCriticality::Idle | LabCriticality::Ok => (0_u64, 1.0_f64),
        LabCriticality::Warn => (16, 1.15),
        LabCriticality::Hot => (30, 1.35),
        LabCriticality::Crit => (45, 1.60),
    };
    samples
        .iter()
        .map(|raw| (((*raw as f64) * gain).round() as u64).clamp(floor, 100))
        .collect()
}

const SPARKLINE_BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub fn sparkline_from_samples(samples: &[u64], width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if samples.is_empty() {
        return "▁".repeat(width);
    }
    let tail = samples.len() - 1;
    let span = width.saturating_sub(1).max(1);
    let top = SPARKLINE_BLOCKS.len() - 1;
    let pinned = width == 1 || tail == 0;
    (0..width)
        .map(|col| {
            let pick = if pinned { tail } else { col.saturating_mul(tail) / span };
            let amp = samples.get(pick).copied().unwrap_or(0).min(100) as usize;
            SPARKLINE_BLOCKS[(amp * top + 50) / 100]
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/vitals.rs"]
mod tests;
