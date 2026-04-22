mod common;

use common::{run_once, MockBackchannel};

#[test]
fn initialize_returns_capabilities() {
    let resp = run_once(
        &MockBackchannel::new(),
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    );
    assert_eq!(resp["id"], 1);
    let result = &resp["result"];
    assert_eq!(result["protocolVersion"], "2024-11-05");
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
    assert_eq!(names.len(), 3);
    for expected in ["emit_signal", "assert_claim", "assert_assumption"] {
        assert!(names.contains(&expected), "missing tool: {expected}");
    }
}

#[test]
fn unknown_method_returns_method_not_found() {
    let resp = run_once(
        &MockBackchannel::new(),
        r#"{"jsonrpc":"2.0","id":9,"method":"does_not_exist","params":{}}"#,
    );
    assert_eq!(resp["error"]["code"], nit_mcp::server::METHOD_NOT_FOUND);
}
