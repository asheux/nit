use std::io::{self, Read, Seek};
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
    // File-backed captures also complete when a wrapper exits but a descendant
    // retains its output handles. Pipe buffers can fill before the child exits.
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut process = ProcessCommand::new(&resolved_exe);
    process
        .args(cli_args)
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    if let Some(augmented_path) = preferred_path_for_executable(&resolved_exe) {
        process.env("PATH", augmented_path);
    }
    let mut child = process.spawn()?;
    let started_at = Instant::now();
    loop {
        if let Some(exit_status) = child.try_wait()? {
            return Ok((
                exit_status,
                read_capture(&mut stdout)?,
                read_capture(&mut stderr)?,
            ));
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

fn read_capture(file: &mut std::fs::File) -> io::Result<Vec<u8>> {
    let size = file.metadata()?.len();
    file.rewind()?;
    let mut bytes = Vec::new();
    file.take(size).read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn captures_output_larger_than_a_pipe_and_bounds_a_silent_child() {
        let (status, stdout, stderr) = run_command_capture_timeout(
            "/bin/sh",
            &[
                "-c",
                "head -c 262144 /dev/zero; head -c 262144 /dev/zero >&2",
            ],
            Duration::from_secs(3),
        )
        .unwrap();
        assert!(status.success());
        assert_eq!((stdout.len(), stderr.len()), (262144, 262144));
        let error = run_command_capture_timeout(
            "/bin/sh",
            &["-c", "exec sleep 8"],
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
