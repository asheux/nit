use super::*;
use crate::snapshot::SnapshotMetadata;
use crate::Rule;

/// Stable metadata fixture; field values do not drift with wall clock
/// so filename / blob assertions remain reproducible.
fn fixture_meta() -> SnapshotMetadata {
    SnapshotMetadata {
        timestamp: "2026-01-25T00:00:00Z".into(),
        workspace_root: None,
        file_path: None,
        seed_source: "test".into(),
        seed_hash: 1,
        rule: "B3/S23".into(),
        rule_id: None,
        protocol: None,
        protocol_hash: None,
        protocol_phase_idx: None,
        protocol_step_in_phase: None,
        generation: 1,
        alive_count: 0,
        period: None,
        score: None,
        wrap_mode: "dead".into(),
        tick_ms: 1,
        attractor: None,
        encoder_id: None,
        encoder_params: None,
        params_fingerprint: None,
        input_hash: None,
        seed_density: None,
        seed_components: None,
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
        seed_hash: 42,
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
        rule_hash: 1,
        seed_hash: 1,
        grid_hash,
        period,
    }
}

/// Two requests with identical content produce equal dedup keys; one
/// with a different grid hash does not.
#[test]
fn snapshot_key_dedupes() {
    let req1 = fixture_request(SnapshotEventKind::Cycle, [1, 2], Some(2));
    let req2 = fixture_request(SnapshotEventKind::Cycle, [1, 2], Some(2));
    let req3 = fixture_request(SnapshotEventKind::Cycle, [1, 3], Some(2));
    assert_eq!(
        SnapshotKey::from_request(&req1),
        SnapshotKey::from_request(&req2),
        "identical content must dedupe"
    );
    assert_ne!(
        SnapshotKey::from_request(&req1),
        SnapshotKey::from_request(&req3),
        "differing grid_hash must not dedupe"
    );
}

/// Non-manual events inside the cooldown window are blocked, but a
/// manual event with a different key still passes.
#[test]
fn cooldown_blocks_non_manual() {
    let now = Instant::now();
    let key = fixture_key([1, 2], Some(2));
    let gate = LastSnapshotKey {
        key: Some(key.clone()),
        last_at: now,
    };
    let cooldown = Duration::from_millis(500);
    let later = now + Duration::from_millis(10);

    assert!(
        !gate.allows(&key, SnapshotEventKind::Cycle, later, cooldown),
        "cycle inside cooldown must be blocked"
    );

    let other_key = fixture_key([3, 4], Some(2));
    assert!(
        gate.allows(&other_key, SnapshotEventKind::Manual, later, cooldown),
        "manual events bypass the cooldown for a distinct key"
    );
}

/// When the bounded channel is full, excess requests are dropped and
/// the dropped counter increments.
#[test]
fn bounded_queue_drops_when_full() {
    let dir = std::env::temp_dir().join("nit-snapshot-test");
    let config = SnapshotManagerConfig {
        dir,
        max_files: 0,
        min_interval_ms: 0,
        queue_capacity: 1,
    };
    let manager = SnapshotManager::new_for_tests(config);
    let req1 = fixture_request(SnapshotEventKind::Manual, [1, 2], None);
    let req2 = fixture_request(SnapshotEventKind::Manual, [3, 4], None);
    assert!(manager.enqueue(req1), "first request fills the queue");
    assert!(
        !manager.enqueue(req2),
        "second request must be dropped when the queue is full"
    );
    let stats = manager.stats();
    assert_eq!(stats.dropped, 1, "exactly one drop recorded");
    assert_eq!(stats.queue_len, 1, "queue still holds the first request");
}
