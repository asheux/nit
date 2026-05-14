use std::path::Path;

/// Parse `.gitignore` at `workspace_root` and return simple directory names.
/// Handles only bare-name patterns; ignores globs, negations, and nested
/// `.gitignore` files. Public so startup can pre-populate
/// `AppState.gitignored_dirs`.
pub fn parse_gitignore_dirs(workspace_root: &Path) -> Vec<String> {
    let gitignore_path = workspace_root.join(".gitignore");
    let Ok(content) = std::fs::read_to_string(&gitignore_path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(parse_gitignore_dir_line)
        .collect()
}

fn parse_gitignore_dir_line(raw: &str) -> Option<String> {
    let line = raw.trim();
    if line.is_empty()
        || line.starts_with('#')
        || line.starts_with('!')
        || line.contains('*')
        || line.contains('?')
    {
        return None;
    }
    let trimmed = line.strip_prefix('/').unwrap_or(line);
    let trimmed = trimmed.strip_suffix('/').unwrap_or(trimmed);
    if trimmed.is_empty() || trimmed.contains('/') {
        return None;
    }
    Some(trimmed.to_string())
}
