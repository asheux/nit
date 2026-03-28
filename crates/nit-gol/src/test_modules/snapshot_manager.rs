use super::*;
use crate::snapshot::SnapshotMetadata;
use crate::Rule;

fn dummy_meta() -> SnapshotMetadata {
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

fn dummy_req(
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
        meta: dummy_meta(),
    }
}

#[test]
fn snapshot_key_dedupes() {
    let req1 = dummy_req(SnapshotEventKind::Cycle, [1, 2], Some(2));
    let req2 = dummy_req(SnapshotEventKind::Cycle, [1, 2], Some(2));
    let req3 = dummy_req(SnapshotEventKind::Cycle, [1, 3], Some(2));
    assert_eq!(
        SnapshotKey::from_request(&req1),
        SnapshotKey::from_request(&req2)
    );
    assert_ne!(
        SnapshotKey::from_request(&req1),
        SnapshotKey::from_request(&req3)
    );
}

#[test]
fn cooldown_blocks_non_manual() {
    let now = Instant::now();
    let key = SnapshotKey {
        event_kind: SnapshotEventKind::Cycle,
        rule_hash: 1,
        seed_hash: 1,
        grid_hash: [1, 2],
        period: Some(2),
    };
    let gate = LastSnapshotKey {
        key: Some(key.clone()),
        last_at: now,
    };
    let later = now + Duration::from_millis(10);
    assert!(!gate.allows(
        &key,
        SnapshotEventKind::Cycle,
        later,
        Duration::from_millis(500)
    ));
    let other_key = SnapshotKey {
        grid_hash: [3, 4],
        ..key
    };
    assert!(gate.allows(
        &other_key,
        SnapshotEventKind::Manual,
        later,
        Duration::from_millis(500)
    ));
}

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
    let req1 = dummy_req(SnapshotEventKind::Manual, [1, 2], None);
    let req2 = dummy_req(SnapshotEventKind::Manual, [3, 4], None);
    assert!(manager.enqueue(req1));
    assert!(!manager.enqueue(req2));
    let stats = manager.stats();
    assert_eq!(stats.dropped, 1);
    assert_eq!(stats.queue_len, 1);
}
