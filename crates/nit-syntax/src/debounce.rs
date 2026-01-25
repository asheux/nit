use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Debouncer {
    delay: Duration,
    last_event: Option<Instant>,
}

impl Debouncer {
    pub fn new(delay_ms: u64) -> Self {
        Self {
            delay: Duration::from_millis(delay_ms),
            last_event: None,
        }
    }

    pub fn mark(&mut self) {
        self.last_event = Some(Instant::now());
    }

    pub fn ready(&self) -> bool {
        match self.last_event {
            Some(t) => t.elapsed() >= self.delay,
            None => false,
        }
    }

    pub fn clear(&mut self) {
        self.last_event = None;
    }

    pub fn delay(&self) -> Duration {
        self.delay
    }
}
