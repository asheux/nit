use std::path::{Path, PathBuf};

use nit_core::{AppState, FileTreeKind, FileTreeRow};

const INDENT_PER_LEVEL: &str = "  ";
const ARROW_EXPANDED: &str = "↓ ";
const ARROW_COLLAPSED: &str = "→ ";
const FILE_INDENT: &str = "  ";

pub fn selected_path(state: &AppState) -> PathBuf {
    state
        .file_tree
        .rows
        .get(state.file_tree.selected)
        .map(|r| r.path.clone())
        .unwrap_or_else(|| state.file_tree.root.clone())
}

pub fn needed_dirs(state: &AppState) -> Vec<PathBuf> {
    let root = &state.file_tree.root;
    let mut expanded: Vec<PathBuf> = state
        .file_tree
        .expanded_dirs
        .iter()
        .filter(|dir| dir.starts_with(root) && dir.as_path() != root.as_path())
        .cloned()
        .collect();
    expanded.sort();

    let mut out = Vec::with_capacity(expanded.len() + 1);
    out.push(root.clone());
    out.extend(expanded);
    out
}

pub fn clear_cache(state: &mut AppState) {
    state.file_tree.cache.clear();
    state.file_tree.loading_dirs.clear();
    state.file_tree.rows.clear();
    state.file_tree.selected = 0;
    state.file_tree.scroll_offset = 0;
}

pub fn rebuild_view(state: &mut AppState, preserve_path: Option<PathBuf>) {
    let root = state.file_tree.root.clone();
    let desired = preserve_path.unwrap_or_else(|| selected_path(state));

    if !state.file_tree.cache.contains_key(&root) {
        let rows = if state.file_tree.open {
            vec![placeholder_root_row(state, &root)]
        } else {
            Vec::new()
        };
        state.file_tree.rows = rows;
        state.file_tree.selected = 0;
        state.file_tree.scroll_offset = 0;
        return;
    }

    let mut rows = Vec::new();
    append_dir(state, &root, 0, &mut rows);

    let fallback_selected = state.file_tree.selected.min(rows.len().saturating_sub(1));
    let new_selected = rows
        .iter()
        .position(|r| r.path == desired)
        .unwrap_or(fallback_selected);
    state.file_tree.selected = new_selected;
    state.file_tree.scroll_offset = if rows.is_empty() {
        0
    } else {
        state
            .file_tree
            .scroll_offset
            .min(rows.len().saturating_sub(1))
            .min(new_selected)
    };
    state.file_tree.rows = rows;
}

fn placeholder_root_row(state: &AppState, root: &Path) -> FileTreeRow {
    let label = if state.file_tree.loading_dirs.contains(root) {
        format!("Loading {}", root.display())
    } else {
        format!("(empty) {}", root.display())
    };
    FileTreeRow {
        text: label,
        path: root.to_path_buf(),
        kind: FileTreeKind::Loading,
        depth: 0,
    }
}

fn append_dir(state: &AppState, dir: &Path, depth: usize, out: &mut Vec<FileTreeRow>) {
    let Some(entries) = state.file_tree.cache.get(dir) else {
        return;
    };
    for entry in entries {
        let expanded = entry.is_dir && state.file_tree.expanded_dirs.contains(&entry.path);
        let still_loading = expanded
            && state.file_tree.loading_dirs.contains(&entry.path)
            && !state.file_tree.cache.contains_key(&entry.path);
        out.push(FileTreeRow {
            text: format_tree_item_label(&entry.name, entry.is_dir, expanded, still_loading, depth),
            path: entry.path.clone(),
            kind: if entry.is_dir {
                FileTreeKind::Dir
            } else {
                FileTreeKind::File
            },
            depth,
        });
        if expanded {
            append_dir(state, &entry.path, depth + 1, out);
        }
    }
}

fn format_tree_item_label(
    name: &str,
    is_dir: bool,
    expanded: bool,
    still_loading: bool,
    depth: usize,
) -> String {
    let arrow = match (is_dir, expanded) {
        (false, _) => FILE_INDENT,
        (true, true) => ARROW_EXPANDED,
        (true, false) => ARROW_COLLAPSED,
    };
    let suffix = match (is_dir, still_loading) {
        (true, true) => "/ (loading)",
        (true, false) => "/",
        _ => "",
    };
    let mut text = String::with_capacity(
        depth * INDENT_PER_LEVEL.len() + arrow.len() + name.len() + suffix.len(),
    );
    for _ in 0..depth {
        text.push_str(INDENT_PER_LEVEL);
    }
    text.push_str(arrow);
    text.push_str(name);
    text.push_str(suffix);
    text
}
