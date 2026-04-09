use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) fn codex_cli_available() -> bool {
    is_executable_in_path("codex")
}

pub(crate) fn claude_cli_available() -> bool {
    is_executable_in_path("claude")
}

pub(crate) fn gemini_cli_available() -> bool {
    is_executable_in_path("gemini")
}

pub(super) fn find_executable_in_path(binary_name: &str) -> Option<PathBuf> {
    for search_dir in executable_search_dirs() {
        if search_dir.as_os_str().is_empty() {
            continue;
        }
        #[cfg(windows)]
        {
            let mut extensions = std::env::var_os("PATHEXT")
                .map(|raw_pathext| {
                    raw_pathext
                        .to_string_lossy()
                        .split(';')
                        .map(|segment| segment.trim())
                        .filter(|segment| !segment.is_empty())
                        .map(|segment| segment.trim_start_matches('.').to_ascii_lowercase())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| vec!["exe".into(), "cmd".into(), "bat".into()]);
            if extensions.is_empty() {
                extensions = vec!["exe".into(), "cmd".into(), "bat".into()];
            }

            let bare_path = search_dir.join(binary_name);
            if bare_path.is_file() {
                return Some(bare_path);
            }
            for ext in &extensions {
                let with_extension = search_dir.join(format!("{binary_name}.{ext}"));
                if with_extension.is_file() {
                    return Some(with_extension);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let full_path = search_dir.join(binary_name);
            if full_path.is_file() {
                return Some(full_path);
            }
        }
    }
    None
}

pub(super) fn probe_models_from_cli(
    binary_name: &str,
    arg_sets: &[&[&str]],
) -> (Vec<String>, Option<String>) {
    let timeout = Duration::from_millis(1500);
    let mut latest_error: Option<String> = None;

    for attempt_args in arg_sets {
        let (exit_status, raw_stdout, raw_stderr) =
            match run_command_capture_timeout(binary_name, attempt_args, timeout) {
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

fn is_executable_in_path(binary_name: &str) -> bool {
    find_executable_in_path(binary_name).is_some()
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
        thread::sleep(Duration::from_millis(25));
    }
}

fn executable_search_dirs() -> Vec<PathBuf> {
    let mut locations = Vec::new();
    if let Some(system_path) = std::env::var_os("PATH") {
        locations.extend(std::env::split_paths(&system_path));
    }
    if let Some(home_os) = std::env::var_os("HOME") {
        let home_root = PathBuf::from(home_os);
        locations.push(home_root.join(".local/bin"));
        locations.push(home_root.join("bin"));
    }

    #[cfg(target_os = "macos")]
    {
        locations.push(PathBuf::from("/opt/homebrew/bin"));
        locations.push(PathBuf::from("/opt/homebrew/sbin"));
    }

    locations.push(PathBuf::from("/usr/local/bin"));
    locations.push(PathBuf::from("/usr/local/sbin"));
    dedup_paths(locations)
}

fn preferred_path_for_executable(resolved_exe: &Path) -> Option<OsString> {
    let mut combined = Vec::new();
    if let Some(parent_dir) = resolved_exe.parent() {
        combined.push(parent_dir.to_path_buf());
    }
    combined.extend(executable_search_dirs());
    std::env::join_paths(dedup_paths(combined)).ok()
}

fn dedup_paths(candidates: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = Vec::with_capacity(candidates.len());
    for entry in candidates {
        if entry.as_os_str().is_empty() || seen.contains(&entry) {
            continue;
        }
        seen.push(entry);
    }
    seen
}

fn parse_model_list_from_output(raw_stdout: &[u8]) -> Vec<String> {
    let decoded_output = String::from_utf8_lossy(raw_stdout);
    let trimmed_content = decoded_output.trim();
    if trimmed_content.is_empty() {
        return Vec::new();
    }

    if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(trimmed_content) {
        let mut collected = Vec::new();
        extract_models_from_json(&json_value, &mut collected);
        collected.sort();
        collected.dedup();
        return collected;
    }

    let mut model_ids = Vec::new();
    for output_line in trimmed_content.lines() {
        let cleaned = output_line
            .trim()
            .trim_start_matches(['-', '*', '•'])
            .trim();
        if cleaned.is_empty() {
            continue;
        }
        let Some(first_token) = cleaned.split_whitespace().next() else {
            continue;
        };
        if first_token.ends_with(':') || first_token.len() < 3 {
            continue;
        }
        if first_token.eq_ignore_ascii_case("models") || first_token.eq_ignore_ascii_case("model") {
            continue;
        }
        model_ids.push(first_token.to_string());
    }
    model_ids.sort();
    model_ids.dedup();
    model_ids
}

fn extract_models_from_json(json_node: &serde_json::Value, collector: &mut Vec<String>) {
    match json_node {
        serde_json::Value::String(text) => {
            let cleaned = text.trim();
            if !cleaned.is_empty() {
                collector.push(cleaned.to_string());
            }
        }
        serde_json::Value::Array(elements) => {
            for child in elements {
                extract_models_from_json(child, collector);
            }
        }
        serde_json::Value::Object(fields) => {
            if let Some(identity_value) = first_identity_field(fields) {
                collector.push(identity_value);
                return;
            }
            for container_key in ["models", "data"] {
                if let Some(nested_value) = fields.get(container_key) {
                    extract_models_from_json(nested_value, collector);
                }
            }
        }
        _ => {}
    }
}

fn first_identity_field(fields: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    for field_name in ["id", "name", "model", "slug"] {
        let Some(serde_json::Value::String(text)) = fields.get(field_name) else {
            continue;
        };
        let cleaned = text.trim();
        if !cleaned.is_empty() {
            return Some(cleaned.to_string());
        }
    }
    None
}
