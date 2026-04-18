use super::*;
use crate::snapshot::SnapshotMetadata;
use crate::Rule;

const FIXED_TIMESTAMP: &str = "2026-01-25T00:00:00Z";
const FIXED_SEED_HASH: u64 = 42;
const FIXED_RULE_HASH: u64 = 1;

/// Metadata fixture with stable fields so filename/blob assertions do
/// not drift with the wall clock.
fn fixture_meta() -> SnapshotMetadata {
    SnapshotMetadata {
        timestamp: FIXED_TIMESTAMP.into(),
        seed_source: "test".into(),
        seed_hash: 1,
        rule: "B3/S23".into(),
        generation: 1,
        alive_count: 0,
        wrap_mode: "dead".into(),
        tick_ms: 1,
        ..Default::default()
    }
}

fn fixture_request(
    event: SnapshotEventKind,
    grid_hash: [u64; 2],
    period: Option<u64>,
) -> SnapshotRequest {
    SnapshotRequest {
        event,
        timestamp: SystemTime::now(),
        gen: 1,
        rule: Rule::conway().to_string(),
        width: 2,
        height: 2,
        wrap: EdgeMode::Dead,
        seed_hash: FIXED_SEED_HASH,
        grid_hash,
        grid_bits: vec![0],
        period,
        transient: None,
        score: None,
        meta: fixture_meta(),
    }
}

fn fixture_key(grid_hash: [u64; 2], period: Option<u64>) -> SnapshotKey {
    SnapshotKey {
        event_kind: SnapshotEventKind::Cycle,
        rule_hash: FIXED_RULE_HASH,
        seed_hash: 1,
        grid_hash,
        period,
    }
}

fn gate_holding(key: SnapshotKey, last_at: Instant) -> LastSnapshotKey {
    LastSnapshotKey {
        key: Some(key),
        last_at,
    }
}

/// Two requests with identical content collapse to the same dedup key;
/// changing the grid_hash alone breaks the equivalence.
#[test]
fn snapshot_key_dedupes() {
    let baseline = fixture_request(SnapshotEventKind::Cycle, [1, 2], Some(2));
    let identical = fixture_request(SnapshotEventKind::Cycle, [1, 2], Some(2));
    let differing_grid = fixture_request(SnapshotEventKind::Cycle, [1, 3], Some(2));

    assert_eq!(
        SnapshotKey::from_request(&baseline),
        SnapshotKey::from_request(&identical),
        "identical content must dedupe",
    );
    assert_ne!(
        SnapshotKey::from_request(&baseline),
        SnapshotKey::from_request(&differing_grid),
        "differing grid_hash must not dedupe",
    );
}

/// Inside the cooldown window, repeat keys are blocked unless the
/// caller marks the event as a Manual capture.
#[test]
fn cooldown_blocks_non_manual() {
    let gate_anchor = Instant::now();
    let cooldown = Duration::from_millis(500);
    let inside_window = gate_anchor + Duration::from_millis(10);

    let recent_key = fixture_key([1, 2], Some(2));
    let gate = gate_holding(recent_key.clone(), gate_anchor);

    assert!(
        !gate.allows(
            &recent_key,
            SnapshotEventKind::Cycle,
            inside_window,
            cooldown
        ),
        "cycle inside cooldown must be blocked",
    );

    let unrelated_key = fixture_key([3, 4], Some(2));
    assert!(
        gate.allows(
            &unrelated_key,
            SnapshotEventKind::Manual,
            inside_window,
            cooldown
        ),
        "manual events bypass the cooldown for a distinct key",
    );
}

/// An empty gate (no prior key) admits the first request immediately,
/// regardless of the cooldown window — seeding from `Instant::now()`
/// minus the cooldown would otherwise leave the first caller starved.
#[test]
fn empty_gate_admits_first_request() {
    let now = Instant::now();
    let cooldown = Duration::from_millis(500);
    let gate = LastSnapshotKey {
        key: None,
        last_at: now,
    };

    let key = fixture_key([1, 2], Some(2));
    assert!(
        gate.allows(&key, SnapshotEventKind::Cycle, now + cooldown, cooldown),
        "first request after gate-anchor cooldown must be admitted",
    );
    assert!(
        gate.allows(&key, SnapshotEventKind::Manual, now, cooldown),
        "manual events must be admitted on an empty gate",
    );
}

/// When the bounded channel is full, excess requests are dropped and
/// the dropped counter increments by exactly one.
#[test]
fn bounded_queue_drops_when_full() {
    let dir = std::env::temp_dir().join("nit-snapshot-test");
    let manager = SnapshotManager::new_for_tests(SnapshotManagerConfig {
        dir,
        max_files: 0,
        min_interval_ms: 0,
        queue_capacity: 1,
    });
    let first = fixture_request(SnapshotEventKind::Manual, [1, 2], None);
    let overflow = fixture_request(SnapshotEventKind::Manual, [3, 4], None);

    assert!(manager.enqueue(first), "first request fills the queue");
    assert!(
        !manager.enqueue(overflow),
        "second request must be dropped when the queue is full",
    );

    let stats = manager.stats();
    assert_eq!(stats.dropped, 1, "exactly one drop recorded");
    assert_eq!(stats.queue_len, 1, "queue still holds the first request");
}
