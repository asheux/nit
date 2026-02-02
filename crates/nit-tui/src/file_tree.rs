use std::collections::HashSet;
use std::path::{Path, PathBuf};

use nit_core::{AppState, FileTreeKind, FileTreeRow};

pub fn selected_path(state: &AppState) -> PathBuf {
    state
        .file_tree
        .rows
        .get(state.file_tree.selected)
        .map(|r| r.path.clone())
        .unwrap_or_else(|| state.file_tree.root.clone())
}

pub fn needed_dirs(state: &AppState) -> Vec<PathBuf> {
    let root = state.file_tree.root.clone();
    let anchor = anchor_dir(state);

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut cur = anchor;
    loop {
        if !cur.starts_with(&root) {
            break;
        }
        if seen.insert(cur.clone()) {
            out.push(cur.clone());
        }
        if cur == root {
            break;
        }
        let Some(parent) = cur.parent() else {
            break;
        };
        cur = parent.to_path_buf();
    }
    if seen.insert(root.clone()) {
        out.push(root);
    }
    out
}

pub fn anchor_dir(state: &AppState) -> PathBuf {
    let root = state.file_tree.root.clone();
    let selected = selected_path(state);
    let selected_kind = state
        .file_tree
        .rows
        .get(state.file_tree.selected)
        .map(|r| r.kind)
        .unwrap_or(FileTreeKind::Dir);
    match selected_kind {
        FileTreeKind::Dir => selected,
        FileTreeKind::File => selected.parent().unwrap_or(root.as_path()).to_path_buf(),
        FileTreeKind::Loading => root,
    }
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

    let mut rows = Vec::new();
    if !state.file_tree.cache.contains_key(&root) {
        if state.file_tree.open {
            let label = if state.file_tree.loading_dirs.contains(&root) {
                format!("Loading {}", root.display())
            } else {
                format!("(empty) {}", root.display())
            };
            rows.push(FileTreeRow {
                text: label,
                path: root.clone(),
                kind: FileTreeKind::Loading,
                depth: 0,
            });
        }
        state.file_tree.rows = rows;
        state.file_tree.selected = 0;
        state.file_tree.scroll_offset = 0;
        return;
    }

    append_dir(state, &root, 0, &desired, &mut rows);

    let new_selected = rows
        .iter()
        .position(|r| r.path == desired)
        .unwrap_or_else(|| state.file_tree.selected.min(rows.len().saturating_sub(1)));
    state.file_tree.rows = rows;
    state.file_tree.selected = new_selected;
    if state.file_tree.rows.is_empty() {
        state.file_tree.scroll_offset = 0;
    } else {
        state.file_tree.scroll_offset = state
            .file_tree
            .scroll_offset
            .min(state.file_tree.rows.len().saturating_sub(1))
            .min(state.file_tree.selected);
    }
}

fn append_dir(
    state: &AppState,
    dir: &Path,
    depth: usize,
    selected_path: &Path,
    out: &mut Vec<FileTreeRow>,
) {
    let Some(entries) = state.file_tree.cache.get(dir) else {
        return;
    };
    for entry in entries {
        let expanded = entry.is_dir && selected_path.starts_with(&entry.path);
        let mut text = String::new();
        for _ in 0..depth {
            text.push_str("  ");
        }
        if entry.is_dir {
            text.push_str(if expanded { "v " } else { "> " });
        } else {
            text.push_str("  ");
        }
        text.push_str(&entry.name);
        if entry.is_dir {
            text.push('/');
            if expanded
                && state.file_tree.loading_dirs.contains(&entry.path)
                && !state.file_tree.cache.contains_key(&entry.path)
            {
                text.push_str(" (loading)");
            }
        }
        out.push(FileTreeRow {
            text,
            path: entry.path.clone(),
            kind: if entry.is_dir {
                FileTreeKind::Dir
            } else {
                FileTreeKind::File
            },
            depth,
        });

        if expanded {
            append_dir(state, &entry.path, depth + 1, selected_path, out);
        }
    }
}
