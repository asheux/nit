// Each tests/*.rs file is its own crate that pulls this module in, so helpers
// unused by a given test binary must not trigger the dead-code lint.
#![allow(dead_code)]

use std::sync::Mutex;

use serde_json::Value;

use nit_mcp::protocol::{BackchannelRequest, BackchannelResponse};
use nit_mcp::server::handle_line;
use nit_mcp::Backchannel;

pub struct MockBackchannel {
    captured: Mutex<Vec<BackchannelRequest>>,
    reply_ok: bool,
}

impl MockBackchannel {
    pub fn new() -> Self {
        Self {
            captured: Mutex::new(Vec::new()),
            reply_ok: true,
        }
    }

    pub fn captured(&self) -> Vec<BackchannelRequest> {
        self.captured.lock().unwrap().clone()
    }
}

impl Backchannel for MockBackchannel {
    fn send(&self, req: &BackchannelRequest) -> anyhow::Result<BackchannelResponse> {
        let request_id = match req {
            BackchannelRequest::EmitSignal { request_id, .. }
            | BackchannelRequest::AssertClaim { request_id, .. }
            | BackchannelRequest::AssertAssumption { request_id, .. } => *request_id,
        };
        self.captured.lock().unwrap().push(req.clone());
        Ok(BackchannelResponse {
            request_id,
            ok: self.reply_ok,
            error: None,
        })
    }
}

pub fn run_once(mock: &MockBackchannel, request: &str) -> Value {
    let mut counter = 0u64;
    let line = handle_line(request, mock, "test-agent", &mut counter);
    serde_json::from_str(&line).unwrap()
}
