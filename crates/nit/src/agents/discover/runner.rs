use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::parse::parse_model_list_from_output;
use super::path::{find_executable_in_path, preferred_path_for_executable};

const SUBPROCESS_TIMEOUT: Duration = Duration::from_millis(1500);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

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
        let (exit_status, raw_stdout, raw_stderr) =
            match run_command_capture_timeout(binary_name, attempt_args, SUBPROCESS_TIMEOUT) {
                Ok(captured) => captured,
                Err(spawn_err) => {
                    latest_error = Some(spawn_err.to_string());
                    continue;
                }
            };

        if !exit_status.success() {
            latest_error = Some(stderr_or_fallback(
                &raw_stderr,
                format!(
                    "{binary_name} {} exited with {exit_status}",
                    attempt_args.join(" ")
                ),
            ));
            continue;
        }

        let discovered_models = parse_model_list_from_output(&raw_stdout);
        if !discovered_models.is_empty() {
            return (discovered_models, None);
        }

        latest_error = Some(stderr_or_fallback(
            &raw_stderr,
            format!(
                "{binary_name} {} returned no models",
                attempt_args.join(" ")
            ),
        ));
    }

    (Vec::new(), latest_error)
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
            let mut captured_stdout = Vec::new();
            let mut captured_stderr = Vec::new();
            if let Some(mut out_pipe) = child.stdout.take() {
                let _ = out_pipe.read_to_end(&mut captured_stdout);
            }
            if let Some(mut err_pipe) = child.stderr.take() {
                let _ = err_pipe.read_to_end(&mut captured_stderr);
            }
            return Ok((exit_status, captured_stdout, captured_stderr));
        }

        if started_at.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "{binary_name} {} timed out after {timeout:?}",
                    cli_args.join(" ")
                ),
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}
