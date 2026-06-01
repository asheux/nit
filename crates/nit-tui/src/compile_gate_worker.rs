//! Background `cargo check` runner that fires after writer turns finish.
//!
//! Closes the operator-reported parallel-swarm gap where one integrator
//! adds a call-site change (e.g. a 6-arg call to a 5-arg function)
//! while a different integrator owns the callee and never updates the
//! signature. Pre-gate, the diff lands in `genome_turn_modified` /
//! `genome_mission_modified` and the genome eval batch finalises with
//! no compile attempt — the error only surfaces when the operator runs
//! `cargo run` after the swarm reports DONE.
//!
//! Shape mirrors `genome_worker.rs`: short-lived threads, one
//! `CompileCheckResult` per check, returned via mpsc. The main loop
//! drains results in `app::runner`, attributes errors per file to the
//! agent that wrote that file (via `genome_turn_modified` /
//! `genome_mission_modified` / substrate claims — same lookup the
//! genome retry path uses), and dispatches a retry continuation to
//! that agent with a budget capped by `COMPILE_GATE_RETRY_LIMIT`.
//!
//! Best-effort: a cargo invocation that itself fails (missing binary,
//! sandbox restrictions) is reported as a `spawn_error` and the caller
//! treats the gate as skipped. The genome retry path remains the
//! quality-side safety net.

use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};

#[derive(Clone, Debug)]
pub struct CompileError {
    /// Workspace-relative path of the file the compiler annotated, e.g.
    /// `crates/nit-tui/src/app/fuzzy_runtime.rs`. Falls back to the
    /// absolute path when stripping the workspace root fails.
    pub rel_path: String,
    pub line: u32,
    pub col: u32,
    /// First line of the diagnostic (the `error[Exxxx]: ...` portion).
    /// Multi-line context is dropped — enough for the retry prompt to
    /// point the agent at the right code.
    pub message: String,
}

#[derive(Debug)]
pub struct CompileCheckResult {
    /// Crate(s) `cargo check` was invoked with. Used by the drain
    /// loop to debounce repeat checks for the same crate set.
    pub crates: Vec<String>,
    pub errors: Vec<CompileError>,
    /// Agent whose `TurnCompleted` triggered this check. Used as the
    /// fallback retry target when no file-owner can be resolved for an
    /// error (e.g. a workspace-level error with no file annotation).
    pub triggering_agent_id: String,
    pub mission_id: Option<String>,
    /// Set when the cargo invocation itself failed (binary missing,
    /// permission denied, etc.). The drain loop treats this as a
    /// silent skip — the gate degrades to no-op rather than blocking
    /// dispatch.
    pub spawn_error: Option<String>,
}

pub struct CompileGateWorker {
    tx: Sender<CompileCheckResult>,
    pub rx: Receiver<CompileCheckResult>,
}

impl Default for CompileGateWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl CompileGateWorker {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx }
    }

    /// Spawn a one-shot thread that runs `cargo check -p <crate>...`
    /// from `workspace_root`. Returns `false` only if the thread spawn
    /// itself fails — the caller decrements any in-flight bookkeeping
    /// in that case.
    pub fn check(
        &self,
        workspace_root: PathBuf,
        crates: Vec<String>,
        triggering_agent_id: String,
        mission_id: Option<String>,
    ) -> bool {
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("compile-gate".into())
            .spawn(move || {
                let result =
                    run_cargo_check(workspace_root, crates, triggering_agent_id, mission_id);
                let _ = tx.send(result);
            })
            .is_ok()
    }
}

fn run_cargo_check(
    workspace_root: PathBuf,
    crates: Vec<String>,
    triggering_agent_id: String,
    mission_id: Option<String>,
) -> CompileCheckResult {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&workspace_root);
    cmd.arg("check");
    cmd.arg("--quiet");
    cmd.arg("--message-format=short");
    for c in &crates {
        cmd.arg("-p");
        cmd.arg(c);
    }
    // Inherit cwd-resolved cargo + offline-friendly flags from the
    // user's env. Don't set CARGO_TARGET_DIR here — we want to reuse
    // the workspace's warm cache so the check completes in seconds,
    // not minutes.
    match cmd.output() {
        Ok(output) if output.status.success() => CompileCheckResult {
            crates,
            errors: Vec::new(),
            triggering_agent_id,
            mission_id,
            spawn_error: None,
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let errors = parse_cargo_short_errors(&stderr, &workspace_root);
            CompileCheckResult {
                crates,
                errors,
                triggering_agent_id,
                mission_id,
                spawn_error: None,
            }
        }
        Err(e) => CompileCheckResult {
            crates,
            errors: Vec::new(),
            triggering_agent_id,
            mission_id,
            spawn_error: Some(format!("spawn cargo check: {e}")),
        },
    }
}

/// Parse `cargo check --message-format=short` output.
///
/// Format observed in practice (rustc emits these annotations to
/// stderr; cargo passes them through verbatim under `--message-format=short`):
///
///   `crates/nit-tui/src/app/fuzzy_runtime.rs:238:22: error[E0061]: this method takes 5 arguments but 6 arguments were supplied`
///
/// Workspace-level errors without a file annotation
/// (`error: could not compile ...`) are dropped — they're aggregations
/// of the per-file errors below them and adding them to the retry
/// prompt is noise. The first 16 file-annotated errors survive; rustc
/// rarely emits more before stopping, and capping prevents a single
/// runaway turn from spamming every agent with the full diagnostic
/// stream.
fn parse_cargo_short_errors(stderr: &str, workspace_root: &std::path::Path) -> Vec<CompileError> {
    const MAX_ERRORS: usize = 16;
    let mut out = Vec::new();
    for line in stderr.lines() {
        if out.len() >= MAX_ERRORS {
            break;
        }
        // Look for `<path>:<line>:<col>: error: <msg>` or
        // `<path>:<line>:<col>: error[Exxxx]: <msg>`. Skip warnings
        // and notes — only errors block the retry.
        let Some((annotated, rest)) = split_first_colon_after_path(line) else {
            continue;
        };
        if !rest.starts_with(" error") {
            continue;
        }
        let Some((path_str, line_col)) = annotated.rsplit_once(':') else {
            continue;
        };
        let Some((path_str, line_str)) = path_str.rsplit_once(':') else {
            continue;
        };
        let Ok(line_num) = line_str.parse::<u32>() else {
            continue;
        };
        let Ok(col_num) = line_col.parse::<u32>() else {
            continue;
        };
        let rel = strip_workspace_prefix(path_str, workspace_root);
        out.push(CompileError {
            rel_path: rel,
            line: line_num,
            col: col_num,
            message: rest.trim_start().to_string(),
        });
    }
    out
}

/// Splits at the third colon — after `path:LINE:COL`. Returns
/// `(path:LINE:COL, " error: ...")`. Needed because path components
/// can contain colons on some hosts; `rsplit_once` from the back
/// hits the `error:` colon, not the `LINE:COL` one.
fn split_first_colon_after_path(line: &str) -> Option<(&str, &str)> {
    let mut count = 0usize;
    let mut idx = None;
    for (i, c) in line.char_indices() {
        if c == ':' {
            count += 1;
            if count == 3 {
                idx = Some(i);
                break;
            }
        }
    }
    let i = idx?;
    Some((&line[..i], &line[i + 1..]))
}

fn strip_workspace_prefix(path: &str, workspace_root: &std::path::Path) -> String {
    let p = std::path::Path::new(path);
    if let Ok(rel) = p.strip_prefix(workspace_root) {
        return rel.to_string_lossy().into_owned();
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_file_line_col_from_short_format() {
        let workspace = PathBuf::from("/Users/op/nit");
        let stderr = "\
/Users/op/nit/crates/nit-tui/src/app/fuzzy_runtime.rs:238:22: error[E0061]: this method takes 5 arguments but 6 arguments were supplied
error: could not compile `nit-tui` (lib) due to 1 previous error
";
        let errors = parse_cargo_short_errors(stderr, &workspace);
        assert_eq!(errors.len(), 1, "workspace-level error must be dropped");
        let e = &errors[0];
        assert_eq!(e.rel_path, "crates/nit-tui/src/app/fuzzy_runtime.rs");
        assert_eq!(e.line, 238);
        assert_eq!(e.col, 22);
        assert!(
            e.message.contains("E0061"),
            "first-line message preserved: {}",
            e.message
        );
    }

    #[test]
    fn drops_warnings_and_notes() {
        let workspace = PathBuf::from("/tmp/w");
        let stderr = "\
/tmp/w/a.rs:1:1: warning: unused import
/tmp/w/b.rs:2:3: note: in module
/tmp/w/c.rs:5:7: error: undeclared identifier
";
        let errors = parse_cargo_short_errors(stderr, &workspace);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].rel_path, "c.rs");
    }

    #[test]
    fn caps_at_max_errors_per_check() {
        let workspace = PathBuf::from("/tmp/w");
        let mut lines = String::new();
        for i in 1..=32 {
            lines.push_str(&format!("/tmp/w/f{i}.rs:1:1: error: bad\n"));
        }
        let errors = parse_cargo_short_errors(&lines, &workspace);
        assert_eq!(errors.len(), 16, "MAX_ERRORS = 16 cap");
    }

    #[test]
    fn keeps_absolute_path_when_not_under_workspace() {
        let workspace = PathBuf::from("/tmp/w");
        let stderr = "/elsewhere/x.rs:1:1: error: bad\n";
        let errors = parse_cargo_short_errors(stderr, &workspace);
        assert_eq!(errors[0].rel_path, "/elsewhere/x.rs");
    }
}
