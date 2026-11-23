mod common;

use common::{run_once, MockBackchannel};
use nit_mcp::jsonrpc::METHOD_NOT_FOUND;

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

#[test]
fn initialize_returns_capabilities() {
    let resp = run_once(
        &MockBackchannel::new(),
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    );
    assert_eq!(resp["id"], 1);
    let result = &resp["result"];
    assert_eq!(result["protocolVersion"], MCP_PROTOCOL_VERSION);
    assert!(result["capabilities"]["tools"].is_object());
    assert_eq!(result["serverInfo"]["name"], "nit-mcp");
}

#[test]
fn tools_list_exposes_all_substrate_tools() {
    let resp = run_once(
        &MockBackchannel::new(),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    );
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 4);
    for expected in [
        "nit_skill",
        "emit_signal",
        "assert_claim",
        "assert_assumption",
    ] {
        assert!(names.contains(&expected), "missing tool: {expected}");
    }
}

#[test]
fn unknown_method_returns_method_not_found() {
    let resp = run_once(
        &MockBackchannel::new(),
        r#"{"jsonrpc":"2.0","id":9,"method":"does_not_exist","params":{}}"#,
    );
    assert_eq!(resp["error"]["code"], METHOD_NOT_FOUND);
}

#[test]
fn skill_lookup_lists_summaries_and_loads_only_the_requested_body() {
    let bc = MockBackchannel::new();
    for (args, expected) in [
        (serde_json::json!({}), "genome-quality"),
        (
            serde_json::json!({"name": "genome-encoders"}),
            "ENCODER GUIDE",
        ),
    ] {
        let request = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "nit_skill", "arguments": args}
        });
        let reply = run_once(&bc, &request.to_string());
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(expected));
        assert!(!text.contains("EQUILIBRIUM RULE"));
    }
    assert!(bc.captured().is_empty());
}
