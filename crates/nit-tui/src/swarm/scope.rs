use std::fs;
use std::path::Path;

pub(super) fn sanitize_for_filename(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Extract directory/module paths from the operator prompt and enumerate their
/// source files.  Returns relative paths sorted alphabetically, capped at 100
/// entries to keep the planner prompt sane.
pub(crate) fn enumerate_scope_files(workspace_root: &Path, prompt: &str) -> Vec<String> {
    // Look for path-like tokens that point to directories inside the workspace.
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    for token in prompt.split_whitespace() {
        let token = token.trim_matches(|c: char| c == ',' || c == '.' || c == '"' || c == '\'');
        if token.is_empty() {
            continue;
        }
        // Must look like a path (contains / or starts with "crates/", "src/", etc.)
        if !token.contains('/') {
            continue;
        }
        let candidate = workspace_root.join(token);
        if candidate.is_dir() {
            dirs.push(candidate);
        }
    }
    if dirs.is_empty() {
        return Vec::new();
    }

    let mut files = Vec::new();
    for dir in dirs.iter() {
        collect_source_files(dir, workspace_root, &mut files);
    }
    files.sort();
    files.dedup();
    files.truncate(100);
    files
}

fn collect_source_files(dir: &Path, workspace_root: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip hidden dirs and target/
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') || name == "target" {
                    continue;
                }
            }
            collect_source_files(&path, workspace_root, out);
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(
                    ext,
                    "rs" | "toml" | "ts" | "js" | "py" | "go" | "c" | "h" | "cpp" | "hpp"
                ) {
                    if let Ok(rel) = path.strip_prefix(workspace_root) {
                        out.push(rel.display().to_string());
                    }
                }
            }
        }
    }
}
