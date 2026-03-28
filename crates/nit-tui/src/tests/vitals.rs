use super::*;

#[test]
fn criticality_boundaries_match_spec_thresholds() {
    let base = CriticalityInput {
        job_running: true,
        hb_age: Some(Duration::from_millis(1900)),
        agent_enabled: false,
        agent_connected: false,
        agent_active_tasks: false,
        ag_age: None,
        recent_errors: 0,
        fatal_error: false,
    };
    assert_eq!(LabCriticality::classify(base), LabCriticality::Ok);
    assert_eq!(
        LabCriticality::classify(CriticalityInput {
            hb_age: Some(Duration::from_secs(2)),
            ..base
        }),
        LabCriticality::Warn
    );
    assert_eq!(
        LabCriticality::classify(CriticalityInput {
            hb_age: Some(Duration::from_secs(5)),
            ..base
        }),
        LabCriticality::Hot
    );
    assert_eq!(
        LabCriticality::classify(CriticalityInput {
            hb_age: Some(Duration::from_secs(10)),
            ..base
        }),
        LabCriticality::Crit
    );
    assert_eq!(
        LabCriticality::classify(CriticalityInput {
            job_running: false,
            hb_age: None,
            ..base
        }),
        LabCriticality::Idle
    );
}

#[test]
fn sampler_flatlines_then_spikes_on_real_event() {
    let mut vitals = VitalsState::new(24);
    let mut now = Instant::now();
    let dt = Duration::from_millis(100);
    let mut idle_last = 0;

    for _ in 0..30 {
        now += dt;
        let snap = vitals.tick(now, dt, false, AgentVitalsState::disabled());
        idle_last = *snap.ecg_samples.last().unwrap_or(&0);
    }
    assert!(
        idle_last <= 20,
        "idle signal should remain low, got {idle_last}"
    );

    vitals.record_job_event(now);
    now += dt;
    let spike_snap = vitals.tick(now, dt, true, AgentVitalsState::disabled());
    let spike = *spike_snap.ecg_samples.last().unwrap_or(&0);
    assert!(spike > idle_last + 35, "expected spike above idle");

    now += dt;
    let decay_snap = vitals.tick(now, dt, true, AgentVitalsState::disabled());
    let decay = *decay_snap.ecg_samples.last().unwrap_or(&0);
    assert!(decay < spike, "signal should decay after spike");
}

#[test]
fn sampler_does_not_pin_full_scale_under_continuous_events() {
    let mut vitals = VitalsState::new(32);
    let mut now = Instant::now();
    let dt = Duration::from_millis(50);
    let mut tail = Vec::new();

    for i in 0..120 {
        now += dt;
        vitals.record_job_event(now);
        let snap = vitals.tick(now, dt, true, AgentVitalsState::disabled());
        if i >= 90 {
            tail.push(*snap.ecg_samples.last().unwrap_or(&0));
        }
    }

    assert!(
        tail.iter().any(|v| *v < 95),
        "continuous events should not lock ECG at full bar: {tail:?}"
    );
    let min = tail.iter().min().copied().unwrap_or(0);
    let max = tail.iter().max().copied().unwrap_or(0);
    assert!(
        max.saturating_sub(min) >= 12,
        "continuous events should still oscillate, tail={tail:?}"
    );
}

#[test]
fn heartbeat_tracker_throttles_to_avoid_zero_age_lock() {
    let mut hb = HeartbeatTracker::default();
    let start = Instant::now();
    hb.record(start);
    hb.record(start + Duration::from_millis(30));

    let age = hb
        .age(start + Duration::from_millis(30))
        .unwrap_or_default();
    assert!(
        age >= Duration::from_millis(30),
        "throttled heartbeat should not refresh too quickly"
    );
}
