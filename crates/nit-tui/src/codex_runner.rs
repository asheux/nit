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
    },
    TurnLog {
        model: String,
        message: String,
    },
    TurnFailed {
        model: String,
        mission_id: Option<String>,
        message: String,
    },
    TurnCompleted {
        model: String,
        mission_id: Option<String>,
        message: String,
    },
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
                reasoning_effort,
                prompt,
            }) => {
                let _ = event_tx.send(CodexEvent::TurnStarted {
                    model: model.clone(),
                    mission_id: mission_id.clone(),
                });
                seq = seq.wrapping_add(1);
                run_turn(
                    &event_tx,
                    seq,
                    model,
                    cwd,
                    mission_id,
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
    reasoning_effort: Option<String>,
    prompt: String,
) {
    let out_file = std::env::temp_dir().join(format!("nit-codex-last-message-{seq}.txt"));

    let mut cmd = Command::new("codex");
    cmd.arg("exec")
        .arg("--ephemeral")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .arg("-m")
        .arg(&model);
    if let Some(effort) = reasoning_effort.as_deref() {
        // Override any global config (e.g. `xhigh`) that some models don't support.
        cmd.arg("-c")
            .arg(format!("model_reasoning_effort={:?}", effort.trim()));
    }
    cmd.arg("-C")
        .arg(&cwd)
        .arg("-o")
        .arg(&out_file)
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
        let _ = event_tx.send(CodexEvent::TurnFailed {
            model,
            mission_id,
            message,
        });
        let _ = std::fs::remove_file(&out_file);
        return;
    }

    let message = std::fs::read_to_string(&out_file).unwrap_or_default();
    let _ = std::fs::remove_file(&out_file);
    let message = message.trim_end().to_string();
    if message.is_empty() {
        let _ = event_tx.send(CodexEvent::TurnFailed {
            model,
            mission_id,
            message: "Codex finished but produced an empty last message.".into(),
        });
        return;
    }

    let _ = event_tx.send(CodexEvent::TurnCompleted {
        model,
        mission_id,
        message,
    });
}
