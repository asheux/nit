//! Process-global queue used to deliver `AgentBusEvent`s that originate
//! outside any runner — primarily the async backend-probe thread spawned by
//! `nit::agents::init_agents` on a cache miss.
//!
//! The TUI's main event loop drains this queue once per tick (alongside the
//! per-runner channels) and applies each event to `AppState`. Producers
//! (e.g. the probe thread) call [`push`]; consumers call [`drain`].
//!
//! The reason for a static queue rather than a passed-through `Sender` is
//! purely architectural: the probe lives in the `nit` crate, the event loop
//! lives in `nit-tui`, and neither can directly hold a channel owned by the
//! other without reshaping `bootstrap` and `multipane_setup` signatures.
//! A `Mutex<Vec<_>>` here is contention-free in practice — probe pushes a
//! handful of events; the event loop drains 20× per second.

use std::sync::{Mutex, OnceLock};

use super::AgentBusEvent;

static QUEUE: OnceLock<Mutex<Vec<AgentBusEvent>>> = OnceLock::new();

fn queue() -> &'static Mutex<Vec<AgentBusEvent>> {
    QUEUE.get_or_init(|| Mutex::new(Vec::new()))
}

/// Enqueue an event for the next event-loop drain. Best-effort: a poisoned
/// mutex silently drops the event — these are non-critical UI-update events
/// and dropping one would only delay the loader-indicator clear by an
/// unbounded but bounded amount (the operator can refresh from the agent
/// ops UI).
pub fn push(event: AgentBusEvent) {
    if let Ok(mut guard) = queue().lock() {
        guard.push(event);
    }
}

/// Take all pending events out of the queue. Returns an empty Vec when the
/// queue is empty or the mutex is poisoned. The event loop calls this once
/// per tick.
#[must_use]
pub fn drain() -> Vec<AgentBusEvent> {
    queue()
        .lock()
        .map(|mut guard| std::mem::take(&mut *guard))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BackendKind;

    fn loaded(seq: u8) -> AgentBusEvent {
        AgentBusEvent::BackendModelsLoaded {
            backend: BackendKind::Claude,
            models: vec![format!("model-{seq}")],
            error: None,
            metadata: None,
        }
    }

    #[test]
    fn push_then_drain_returns_in_order() {
        // Other tests may have left state behind; the queue is a process
        // singleton. Drain first so this test sees a clean slate.
        let _ = drain();

        push(loaded(1));
        push(loaded(2));

        let drained = drain();
        assert_eq!(drained.len(), 2);
        match &drained[0] {
            AgentBusEvent::BackendModelsLoaded { models, .. } => {
                assert_eq!(models, &vec!["model-1".to_string()]);
            }
            _ => panic!("expected BackendModelsLoaded"),
        }
        match &drained[1] {
            AgentBusEvent::BackendModelsLoaded { models, .. } => {
                assert_eq!(models, &vec!["model-2".to_string()]);
            }
            _ => panic!("expected BackendModelsLoaded"),
        }
        // Second drain is empty.
        assert!(drain().is_empty());
    }
}
