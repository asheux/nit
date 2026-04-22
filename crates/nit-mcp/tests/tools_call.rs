mod common;

use common::{run_once, MockBackchannel, TEST_AGENT_ID};
use nit_core::substrate::{ClaimKind, SignalKind, SignalTarget};
use nit_mcp::jsonrpc::{INVALID_PARAMS, METHOD_NOT_FOUND};
use nit_mcp::protocol::BackchannelRequest;

#[test]
fn emit_signal_forwards_fields_to_backchannel() {
    let mock = MockBackchannel::new();
    let resp = run_once(
        &mock,
        r#"{
            "jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                "name":"emit_signal",
                "arguments":{
                    "kind":"warning",
                    "target":{"kind":"global"},
                    "payload":{"msg":"hello"},
                    "strength":0.8
                }
            }
        }"#,
    );
    assert!(resp["error"].is_null(), "unexpected error: {resp}");

    let captured = mock.captured();
    let BackchannelRequest::EmitSignal {
        agent_id,
        kind,
        target,
        payload,
        strength,
        ..
    } = captured.first().expect("one request captured")
    else {
        panic!("expected EmitSignal, got {:?}", captured.first());
    };
    assert_eq!(agent_id, TEST_AGENT_ID);
    assert_eq!(*kind, SignalKind::Warning);
    assert_eq!(*target, SignalTarget::Global);
    assert_eq!(payload["msg"], "hello");
    assert!((strength.unwrap() - 0.8).abs() < f32::EPSILON * 10.0);
}

#[test]
fn assert_claim_forwards_ttl_and_rationale() {
    let mock = MockBackchannel::new();
    let resp = run_once(
        &mock,
        r#"{
            "jsonrpc":"2.0","id":4,"method":"tools/call","params":{
                "name":"assert_claim",
                "arguments":{
                    "kind":"exclusive_write",
                    "target":{"kind":"file","path":"/tmp/foo"},
                    "ttl_gens":5,
                    "rationale":"integration mission"
                }
            }
        }"#,
    );
    assert!(resp["error"].is_null(), "unexpected error: {resp}");

    let captured = mock.captured();
    let BackchannelRequest::AssertClaim {
        agent_id,
        kind,
        ttl_gens,
        rationale,
        ..
    } = captured.first().expect("one request captured")
    else {
        panic!("expected AssertClaim, got {:?}", captured.first());
    };
    assert_eq!(agent_id, TEST_AGENT_ID);
    assert_eq!(*kind, ClaimKind::ExclusiveWrite);
    assert_eq!(*ttl_gens, 5);
    assert_eq!(rationale, "integration mission");
}

#[test]
fn malformed_kind_variant_is_invalid_params() {
    // `nonsense` is not a valid SignalKind variant — serde rejects it.
    let resp = run_once(
        &MockBackchannel::new(),
        r#"{
            "jsonrpc":"2.0","id":5,"method":"tools/call","params":{
                "name":"emit_signal",
                "arguments":{"kind":"nonsense","target":{"kind":"global"}}
            }
        }"#,
    );
    assert_eq!(resp["error"]["code"], INVALID_PARAMS);
}

#[test]
fn unknown_tool_name_is_method_not_found() {
    let resp = run_once(
        &MockBackchannel::new(),
        r#"{
            "jsonrpc":"2.0","id":6,"method":"tools/call","params":{
                "name":"nope","arguments":{}
            }
        }"#,
    );
    assert_eq!(resp["error"]["code"], METHOD_NOT_FOUND);
}
