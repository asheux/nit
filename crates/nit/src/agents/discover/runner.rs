use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::parse::parse_model_list_from_output;
use super::path::{find_executable_in_path, preferred_path_for_executable};

const SUBPROCESS_TIMEOUT: Duration = Duration::from_millis(1500);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

pub(in crate::agents) const DEFAULT_MODEL_LIST_ARG_SETS: &[&[&str]] = &[
    &["models", "--json"],
    &["models"],
    &["list-models"],
    &["--list-models"],
];

pub(in crate::agents) fn capture_cli_help_text(binary_name: &str) -> Option<String> {
    let (exit_status, raw_stdout, _raw_stderr) =
        run_command_capture_timeout(binary_name, &["--help"], SUBPROCESS_TIMEOUT).ok()?;
    if !exit_status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&raw_stdout).into_owned())
}

pub(in crate::agents) fn probe_models_from_cli(
    binary_name: &str,
    arg_sets: &[&[&str]],
) -> (Vec<String>, Option<String>) {
    let mut latest_error: Option<String> = None;
    for attempt_args in arg_sets {
        match attempt_model_probe(binary_name, attempt_args) {
            Ok(models) => return (models, None),
            Err(error) => latest_error = Some(error),
        }
    }
    (Vec::new(), latest_error)
}

fn attempt_model_probe(binary_name: &str, attempt_args: &[&str]) -> Result<Vec<String>, String> {
    let (exit_status, raw_stdout, raw_stderr) =
        run_command_capture_timeout(binary_name, attempt_args, SUBPROCESS_TIMEOUT)
            .map_err(|spawn_err| spawn_err.to_string())?;

    if !exit_status.success() {
        let suffix = format!(
            "{binary_name} {} exited with {exit_status}",
            attempt_args.join(" ")
        );
        return Err(stderr_or_fallback(&raw_stderr, suffix));
    }

    let discovered = parse_model_list_from_output(&raw_stdout);
    if discovered.is_empty() {
        let suffix = format!(
            "{binary_name} {} returned no models",
            attempt_args.join(" ")
        );
        return Err(stderr_or_fallback(&raw_stderr, suffix));
    }
    Ok(discovered)
}

fn stderr_or_fallback(raw_stderr: &[u8], fallback_message: String) -> String {
    let decoded = String::from_utf8_lossy(raw_stderr).trim().to_string();
    if decoded.is_empty() {
        fallback_message
    } else {
        decoded
    }
}

fn run_command_capture_timeout(
    binary_name: &str,
    cli_args: &[&str],
    timeout: Duration,
) -> io::Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
    let resolved_exe =
        find_executable_in_path(binary_name).unwrap_or_else(|| PathBuf::from(binary_name));
    let mut process = ProcessCommand::new(&resolved_exe);
    process
        .args(cli_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(augmented_path) = preferred_path_for_executable(&resolved_exe) {
        process.env("PATH", augmented_path);
    }
    let mut child = process.spawn()?;
    let started_at = Instant::now();
    loop {
        if let Some(exit_status) = child.try_wait()? {
            let stdout = drain_pipe(child.stdout.take());
            let stderr = drain_pipe(child.stderr.take());
            return Ok((exit_status, stdout, stderr));
        }
        if started_at.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let suffix = format!(
                "{binary_name} {} timed out after {timeout:?}",
                cli_args.join(" ")
            );
            return Err(io::Error::new(io::ErrorKind::TimedOut, suffix));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn drain_pipe<R: Read>(pipe: Option<R>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut handle) = pipe {
        let _ = handle.read_to_end(&mut buf);
    }
    buf
}
