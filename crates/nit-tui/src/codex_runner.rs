use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

#[derive(Clone, Debug)]
pub enum CodexCommand {
    RunTurn {
        model: String,
        cwd: PathBuf,
        mission_id: Option<String>,
        /// Resume an existing Codex session/thread id when present.
        resume_thread_id: Option<String>,
        /// When false, run the turn as `--ephemeral` (no on-disk Codex session state).
        /// When true, persist the session so future turns can resume it.
        persist_session: bool,
        reasoning_effort: Option<String>,
        prompt: String,
    },
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum CodexEvent {
    TurnStarted {
        model: String,
        mission_id: Option<String>,
        resume_thread_id: Option<String>,
    },
    TurnLog {
        model: String,
        message: String,
    },
    TurnFailed {
        model: String,
        mission_id: Option<String>,
        thread_id: Option<String>,
        token_count: Option<CodexTokenCount>,
        message: String,
    },
    TurnCompleted {
        model: String,
        mission_id: Option<String>,
        thread_id: Option<String>,
        token_count: Option<CodexTokenCount>,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexTokenCount {
    pub total_tokens: u32,
    pub context_window: u32,
}

pub struct CodexRunner {
    cmd_tx: Sender<CodexCommand>,
    pub events: Receiver<CodexEvent>,
    handle: Option<JoinHandle<()>>,
}

impl CodexRunner {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("nit-codex".into())
            .spawn(move || runner_loop(cmd_rx, event_tx))
            .expect("spawn codex runner");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
        }
    }

    pub fn send(&self, command: CodexCommand) {
        let _ = self.cmd_tx.send(command);
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(CodexCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn runner_loop(cmd_rx: Receiver<CodexCommand>, event_tx: Sender<CodexEvent>) {
    let mut seq = 0u64;
    loop {
        match cmd_rx.recv() {
            Ok(CodexCommand::RunTurn {
                model,
                cwd,
                mission_id,
                resume_thread_id,
                persist_session,
                reasoning_effort,
                prompt,
            }) => {
                let _ = event_tx.send(CodexEvent::TurnStarted {
                    model: model.clone(),
                    mission_id: mission_id.clone(),
                    resume_thread_id: resume_thread_id.clone(),
                });
                seq = seq.wrapping_add(1);
                run_turn(
                    &event_tx,
                    seq,
                    model,
                    cwd,
                    mission_id,
                    resume_thread_id,
                    persist_session,
                    reasoning_effort,
                    prompt,
                );
            }
            Ok(CodexCommand::Shutdown) | Err(_) => break,
        }
    }
}

fn run_turn(
    event_tx: &Sender<CodexEvent>,
    seq: u64,
    model: String,
    cwd: PathBuf,
    mission_id: Option<String>,
    resume_thread_id: Option<String>,
    persist_session: bool,
    reasoning_effort: Option<String>,
    prompt: String,
) {
    let out_file = std::env::temp_dir().join(format!("nit-codex-last-message-{seq}.txt"));

    let mut cmd = Command::new("codex");
    if let Some(_thread_id) = resume_thread_id.as_deref() {
        cmd.arg("exec").arg("resume").arg("--json").arg("-m").arg(&model);
    } else {
        cmd.arg("exec").arg("--json").arg("--color").arg("never");
        if !persist_session {
            cmd.arg("--ephemeral");
        }
        cmd.arg("-m").arg(&model).arg("-C").arg(&cwd);
    }
    if let Some(effort) = reasoning_effort.as_deref() {
        // Override any global config (e.g. `xhigh`) that some models don't support.
        cmd.arg("-c")
            .arg(format!("model_reasoning_effort={:?}", effort.trim()));
    }
    cmd.arg("-o").arg(&out_file);
    if let Some(thread_id) = resume_thread_id.as_deref() {
        // Positional SESSION_ID comes after options for `codex exec resume`.
        cmd.arg(thread_id);
    }
    cmd
        // Read prompt from stdin so multi-line input works without shell escaping.
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
	        Err(err) => {
	            let _ = event_tx.send(CodexEvent::TurnFailed {
	                model,
	                mission_id,
	                thread_id: resume_thread_id.clone(),
	                token_count: None,
	                message: format!("Failed to spawn codex: {err}"),
	            });
	            return;
	        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
        let _ = stdin.write_all(b"\n");
    }

    let output = match child.wait_with_output() {
        Ok(output) => output,
	        Err(err) => {
	            let _ = event_tx.send(CodexEvent::TurnFailed {
	                model,
	                mission_id,
	                thread_id: resume_thread_id.clone(),
	                token_count: None,
	                message: format!("Codex wait failed: {err}"),
	            });
	            let _ = std::fs::remove_file(&out_file);
	            return;
	        }
    };

    // Parse Codex JSONL stdout (best-effort) into diagnostic messages.
    let mut json_errors: Vec<String> = Vec::new();
    for raw in String::from_utf8_lossy(&output.stdout).lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            let _ = event_tx.send(CodexEvent::TurnLog {
                model: model.clone(),
                message: raw.to_string(),
            });
            continue;
        };
        let Some(kind) = value.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        if kind == "error" {
            let msg = value.get("message").and_then(|v| v.as_str()).or_else(|| {
                value
                    .get("error")
                    .and_then(|err| err.get("message"))?
                    .as_str()
            });
            if let Some(msg) = msg {
                json_errors.push(msg.to_string());
                let _ = event_tx.send(CodexEvent::TurnLog {
                    model: model.clone(),
                    message: msg.to_string(),
                });
            }
        }
    }

    // Stderr can contain plain-text warnings even when `--json` is used.
    let stderr_text = String::from_utf8_lossy(&output.stderr);
    for line in stderr_text.lines() {
        let line = line.trim();
        if !line.is_empty() {
            let _ = event_tx.send(CodexEvent::TurnLog {
                model: model.clone(),
                message: line.to_string(),
            });
        }
    }

    if !output.status.success() {
        let message = if !json_errors.is_empty() {
            json_errors.join(" | ")
        } else if !stderr_text.trim().is_empty() {
            stderr_text.trim().to_string()
        } else {
            format!("Codex exited with {}", output.status)
        };
        let thread_id = extract_thread_id_from_jsonl(&output.stdout);
        let token_count = extract_token_count_from_jsonl(&output.stdout);
        let _ = event_tx.send(CodexEvent::TurnFailed {
            model,
            mission_id,
            thread_id,
            token_count,
            message,
        });
        let _ = std::fs::remove_file(&out_file);
        return;
    }

    let message = std::fs::read_to_string(&out_file).unwrap_or_default();
    let _ = std::fs::remove_file(&out_file);
    let message = message.trim_end().to_string();
    if message.is_empty() {
        let thread_id = extract_thread_id_from_jsonl(&output.stdout);
        let token_count = extract_token_count_from_jsonl(&output.stdout);
        let _ = event_tx.send(CodexEvent::TurnFailed {
            model,
            mission_id,
            thread_id,
            token_count,
            message: "Codex finished but produced an empty last message.".into(),
        });
        return;
    }

    let thread_id = extract_thread_id_from_jsonl(&output.stdout);
    let token_count = extract_token_count_from_jsonl(&output.stdout);
    let _ = event_tx.send(CodexEvent::TurnCompleted {
        model,
        mission_id,
        thread_id,
        token_count,
        message,
    });
}

fn extract_thread_id_from_jsonl(stdout: &[u8]) -> Option<String> {
    // `codex exec --json` emits a "thread.started" event with a `thread_id` field.
    let text = String::from_utf8_lossy(stdout);
    for raw in text.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        let Some(thread_id) = value.get("thread_id").and_then(|v| v.as_str()) else {
            continue;
        };
        if thread_id.trim().is_empty() {
            continue;
        }
        // Prefer the canonical thread lifecycle events, but accept any event containing thread_id.
        if let Some(kind) = value.get("type").and_then(|v| v.as_str()) {
            if kind.starts_with("thread.") {
                return Some(thread_id.to_string());
            }
        }
        return Some(thread_id.to_string());
    }
    None
}

fn extract_token_count_from_jsonl(stdout: &[u8]) -> Option<CodexTokenCount> {
    // Codex streams "token_count" events that include total token usage + context window.
    // We accept both exec-mode JSONL and session-style wrapped events.
    let text = String::from_utf8_lossy(stdout);
    let mut last: Option<CodexTokenCount> = None;
    for raw in text.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };

        let payload = value.get("payload").unwrap_or(&value);
        let Some(kind) = payload.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        if kind != "token_count" {
            continue;
        }
        let Some(info) = payload.get("info") else {
            continue;
        };
        let context_window = info
            .get("model_context_window")
            .and_then(|v| v.as_u64())
            .filter(|v| *v > 0)?;
        let total_tokens = info
            .get("total_token_usage")
            .and_then(|u| u.get("total_tokens"))
            .and_then(|v| v.as_u64())
            .or_else(|| {
                info.get("last_token_usage")
                    .and_then(|u| u.get("total_tokens"))
                    .and_then(|v| v.as_u64())
            })?;
        if context_window > u32::MAX as u64 || total_tokens > u32::MAX as u64 {
            continue;
        }
        last = Some(CodexTokenCount {
            total_tokens: total_tokens as u32,
            context_window: context_window as u32,
        });
    }
    last
}

#[cfg(test)]
mod tests {
    use super::{extract_thread_id_from_jsonl, extract_token_count_from_jsonl, CodexTokenCount};

    #[test]
    fn extracts_thread_id_from_event_stream() {
        let jsonl = br#"{"type":"thread.started","thread_id":"019ca7c5-536f-7f81-82a7-7a38fa483cb2"}
{"type":"turn.started"}
{"type":"turn.completed"}"#;
        assert_eq!(
            extract_thread_id_from_jsonl(jsonl).as_deref(),
            Some("019ca7c5-536f-7f81-82a7-7a38fa483cb2")
        );
    }

    #[test]
    fn ignores_empty_thread_id() {
        let jsonl = br#"{"type":"thread.started","thread_id":"  "}
{"type":"turn.started"}"#;
        assert!(extract_thread_id_from_jsonl(jsonl).is_none());
    }

    #[test]
    fn extracts_last_token_count_from_wrapped_events() {
        let jsonl = br#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":100},"model_context_window":1000}}}
{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":250},"model_context_window":1000}}}"#;
        assert_eq!(
            extract_token_count_from_jsonl(jsonl),
            Some(CodexTokenCount {
                total_tokens: 250,
                context_window: 1000
            })
        );
    }
}
