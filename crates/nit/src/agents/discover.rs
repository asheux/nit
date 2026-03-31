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

pub(super) fn find_executable_in_path(bin: &str) -> Option<PathBuf> {
    for dir in executable_search_dirs() {
        if dir.as_os_str().is_empty() {
            continue;
        }
        #[cfg(windows)]
        {
            let mut exts = std::env::var_os("PATHEXT")
                .map(|v| {
                    v.to_string_lossy()
                        .split(';')
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.trim_start_matches('.').to_ascii_lowercase())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| vec!["exe".into(), "cmd".into(), "bat".into()]);
            if exts.is_empty() {
                exts = vec!["exe".into(), "cmd".into(), "bat".into()];
            }

            let candidate = dir.join(bin);
            if candidate.is_file() {
                return Some(candidate);
            }
            for ext in exts.iter() {
                let candidate = dir.join(format!("{bin}.{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let candidate = dir.join(bin);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub(super) fn probe_models_from_cli(
    bin: &str,
    attempts: &[&[&str]],
) -> (Vec<String>, Option<String>) {
    let timeout = Duration::from_millis(1500);
    let mut last_err: Option<String> = None;

    for args in attempts {
        match run_command_capture_timeout(bin, args, timeout) {
            Ok((status, stdout, stderr)) => {
                if !status.success() {
                    let err = String::from_utf8_lossy(&stderr).trim().to_string();
                    last_err = Some(if err.is_empty() {
                        format!("{bin} {} exited with {status}", args.join(" "))
                    } else {
                        err
                    });
                    continue;
                }

                let models = parse_model_list_from_output(&stdout);
                if !models.is_empty() {
                    return (models, None);
                }

                let err = String::from_utf8_lossy(&stderr).trim().to_string();
                last_err = Some(if err.is_empty() {
                    format!("{bin} {} returned no models", args.join(" "))
                } else {
                    err
                });
            }
            Err(err) => {
                last_err = Some(err.to_string());
            }
        }
    }

    (Vec::new(), last_err)
}

fn is_executable_in_path(bin: &str) -> bool {
    find_executable_in_path(bin).is_some()
}

fn run_command_capture_timeout(
    bin: &str,
    args: &[&str],
    timeout: Duration,
) -> io::Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
    let executable = find_executable_in_path(bin).unwrap_or_else(|| PathBuf::from(bin));
    let mut command = ProcessCommand::new(&executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path_override) = preferred_path_for_executable(&executable) {
        command.env("PATH", path_override);
    }
    let mut child = command.spawn()?;

    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            if let Some(mut out) = child.stdout.take() {
                let _ = out.read_to_end(&mut stdout);
            }
            if let Some(mut err) = child.stderr.take() {
                let _ = err.read_to_end(&mut stderr);
            }
            return Ok((status, stdout, stderr));
        }

        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{bin} {} timed out after {timeout:?}", args.join(" ")),
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn executable_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join("bin"));
    }

    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/opt/homebrew/sbin"));
    }

    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/usr/local/sbin"));

    let mut unique = Vec::new();
    for dir in dirs {
        if dir.as_os_str().is_empty() || unique.iter().any(|existing| existing == &dir) {
            continue;
        }
        unique.push(dir);
    }
    unique
}

fn preferred_path_for_executable(executable: &Path) -> Option<OsString> {
    let mut paths = Vec::<PathBuf>::new();
    if let Some(dir) = executable.parent() {
        paths.push(dir.to_path_buf());
    }
    paths.extend(executable_search_dirs());
    let mut deduped = Vec::new();
    for path in paths {
        if deduped.iter().any(|existing| existing == &path) {
            continue;
        }
        deduped.push(path);
    }
    std::env::join_paths(deduped).ok()
}

fn parse_model_list_from_output(stdout: &[u8]) -> Vec<String> {
    let raw = String::from_utf8_lossy(stdout);
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        let mut out = Vec::new();
        extract_models_from_json(&value, &mut out);
        out.sort();
        out.dedup();
        return out;
    }

    let mut out = Vec::new();
    for line in raw.lines() {
        let mut line = line.trim();
        if line.is_empty() {
            continue;
        }
        line = line
            .trim_start_matches('-')
            .trim_start_matches('*')
            .trim_start_matches('•')
            .trim();
        if line.is_empty() {
            continue;
        }
        let Some(candidate) = line.split_whitespace().next() else {
            continue;
        };
        if candidate.ends_with(':') {
            continue;
        }
        if candidate.eq_ignore_ascii_case("models") || candidate.eq_ignore_ascii_case("model") {
            continue;
        }
        if candidate.len() < 3 {
            continue;
        }
        out.push(candidate.to_string());
    }
    out.sort();
    out.dedup();
    out
}

fn extract_models_from_json(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => {
            let s = s.trim();
            if !s.is_empty() {
                out.push(s.to_string());
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                extract_models_from_json(v, out);
            }
        }
        serde_json::Value::Object(map) => {
            for key in ["id", "name", "model", "slug"] {
                if let Some(serde_json::Value::String(s)) = map.get(key) {
                    let s = s.trim();
                    if !s.is_empty() {
                        out.push(s.to_string());
                        return;
                    }
                }
            }
            for key in ["models", "data"] {
                if let Some(v) = map.get(key) {
                    extract_models_from_json(v, out);
                }
            }
        }
        _ => {}
    }
}
