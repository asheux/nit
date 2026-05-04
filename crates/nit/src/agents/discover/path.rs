use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub(in crate::agents) fn find_executable_in_path(binary_name: &str) -> Option<PathBuf> {
    executable_search_dirs()
        .into_iter()
        .filter(|dir| !dir.as_os_str().is_empty())
        .find_map(|dir| find_in_dir(&dir, binary_name))
}

#[cfg(not(windows))]
fn find_in_dir(search_dir: &Path, binary_name: &str) -> Option<PathBuf> {
    let candidate = search_dir.join(binary_name);
    candidate.is_file().then_some(candidate)
}

#[cfg(windows)]
fn find_in_dir(search_dir: &Path, binary_name: &str) -> Option<PathBuf> {
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
    None
}

pub(super) fn preferred_path_for_executable(resolved_exe: &Path) -> Option<OsString> {
    let mut combined = Vec::new();
    if let Some(parent_dir) = resolved_exe.parent() {
        combined.push(parent_dir.to_path_buf());
    }
    combined.extend(executable_search_dirs());
    std::env::join_paths(dedup_paths(combined)).ok()
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

fn dedup_paths(candidates: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::with_capacity(candidates.len());
    candidates
        .into_iter()
        .filter(|entry| !entry.as_os_str().is_empty() && seen.insert(entry.clone()))
        .collect()
}
