//! Unit tests for the `nit-mcp-server` backchannel listener.

use std::sync::mpsc;

use nit_core::AgentBusEvent;
use nit_mcp::protocol::BackchannelRequest;

use crate::mcp_backchannel::{backchannel_to_event, McpBackchannel};

#[test]
fn backchannel_to_event_maps_emit_signal() {
    let req = BackchannelRequest::EmitSignal {
        request_id: 42,
        agent_id: "agent-x".into(),
        kind: nit_core::substrate::SignalKind::Warning,
        target: nit_core::substrate::SignalTarget::Global,
        payload: serde_json::json!({"k": "v"}),
        strength: Some(0.7),
    };
    let (id, event) = backchannel_to_event(req);
    assert_eq!(id, 42);
    match event {
        AgentBusEvent::EmitSignalRequest {
            posted_by,
            initial_strength,
            ..
        } => {
            assert_eq!(posted_by, "agent-x");
            assert!((initial_strength.unwrap() - 0.7).abs() < f32::EPSILON * 10.0);
        }
        other => panic!("expected EmitSignalRequest, got {other:?}"),
    }
}

#[test]
fn backchannel_to_event_maps_assert_claim() {
    let req = BackchannelRequest::AssertClaim {
        request_id: 9,
        agent_id: "agent-y".into(),
        kind: nit_core::substrate::ClaimKind::Soft,
        target: nit_core::substrate::ClaimTarget::Global,
        ttl_gens: 2,
        rationale: "r".into(),
    };
    let (id, event) = backchannel_to_event(req);
    assert_eq!(id, 9);
    assert!(matches!(event, AgentBusEvent::AssertClaimRequest { .. }));
}

#[test]
fn spawn_and_drop_removes_socket() {
    let (tx, _rx) = mpsc::channel();
    let bc = McpBackchannel::spawn(tx).expect("spawn");
    let path = bc.socket_path.clone();
    assert!(std::path::Path::new(&path).exists());
    drop(bc);
    // The listener thread exits when the sender half is dropped, so the
    // socket file is unlinked by Drop before the test ends.
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(!std::path::Path::new(&path).exists());
}
