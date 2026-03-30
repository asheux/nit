use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use nit_core::{io as core_io, Buffer};
use nit_utils::hashing::stable_hash_bytes;
use nit_utils::paths;

/// Try to get the git HEAD version of a file for diff base.
fn git_head_content(path: &Path) -> Option<String> {
    use std::process::Command;
    let dir = path.parent()?;
    // Get the repo-relative path
    let rel = Command::new("git")
        .args(["ls-files", "--full-name", "--error-unmatch"])
        .arg(path)
        .current_dir(dir)
        .output()
        .ok()?;
    if !rel.status.success() {
        return None; // not tracked by git
    }
    let rel_path = String::from_utf8(rel.stdout).ok()?;
    let rel_path = rel_path.trim();
    let output = Command::new("git")
        .args(["show", &format!("HEAD:{rel_path}")])
        .current_dir(dir)
        .output()
        .ok()?;
    if output.status.success() {
        String::from_utf8(output.stdout).ok()
    } else {
        None
    }
}

fn load_file_buffer(path: &Path, default_name: &str) -> anyhow::Result<Buffer> {
    let content = core_io::load_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| default_name.into());
    let mut buffer = Buffer::from_str(name, &content, Some(path.to_path_buf()));
    if let Some(git_base) = git_head_content(path) {
        buffer.set_git_base(&git_base);
    }
    Ok(buffer)
}

fn parent_or_cwd(path: &Path) -> anyhow::Result<PathBuf> {
    path.parent()
        .map(|p| Ok(p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().map_err(Into::into))
}

pub(crate) fn open_target_gol(path: Option<&Path>) -> anyhow::Result<(PathBuf, Buffer)> {
    match path {
        Some(p) if p.is_file() => {
            let buffer = load_file_buffer(p, "untitled")?;
            let root = parent_or_cwd(p)?;
            Ok((root, buffer))
        }
        Some(p) if p.is_dir() => {
            let root = p.to_path_buf();
            let buffer = Buffer::empty("untitled", None);
            Ok((root, buffer))
        }
        None => {
            let root = std::env::current_dir()?;
            let buffer = Buffer::empty("untitled", None);
            Ok((root, buffer))
        }
        Some(other) => anyhow::bail!("path does not exist: {}", other.display()),
    }
}

pub(crate) fn open_target_games(path: Option<&Path>) -> anyhow::Result<(PathBuf, Buffer)> {
    match path {
        Some(p) if p.is_file() => {
            let buffer = load_file_buffer(p, "games.toml")?;
            let root = parent_or_cwd(p)?;
            Ok((root, buffer))
        }
        Some(p) if p.is_dir() => open_games_workspace(p),
        None => {
            let root = std::env::current_dir()?;
            open_games_workspace(&root)
        }
        Some(other) => anyhow::bail!("path does not exist: {}", other.display()),
    }
}

fn open_games_workspace(root: &Path) -> anyhow::Result<(PathBuf, Buffer)> {
    let root = root.to_path_buf();
    let config_path = root.join("games.toml");
    if config_path.exists() {
        let buffer = load_file_buffer(&config_path, "games.toml")?;
        return Ok((root, buffer));
    }
    let buffer = Buffer::from_str(
        "games.toml",
        crate::games::games_template(),
        Some(config_path),
    );
    Ok((root, buffer))
}

pub(crate) fn find_theme() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let local = cwd.join("assets/themes/devs.toml");
    if local.exists() {
        return Some(local);
    }
    None
}

pub(crate) fn load_notes(workspace_root: &Path) -> Buffer {
    let Some(path) = notes_path_for_workspace(workspace_root) else {
        return Buffer::empty("notes", None);
    };
    if path.exists() {
        if let Ok(content) = core_io::load_to_string(&path) {
            return Buffer::from_str("notes", &content, Some(path));
        }
    }
    Buffer::empty("notes", Some(path))
}

fn notes_path_for_workspace(workspace_root: &Path) -> Option<PathBuf> {
    let base = paths::state_dir().or_else(paths::data_dir)?;
    let notes_dir = base.join("notes");
    let _ = fs::create_dir_all(&notes_dir);
    let key = workspace_root.to_string_lossy();
    let hash = stable_hash_bytes(key.as_bytes());
    let filename = format!("{hash:016x}.md");
    Some(notes_dir.join(filename))
}

pub(crate) fn export_legacy_notes_snapshot(
    workspace_root: &Path,
    buffer: &Buffer,
) -> Option<PathBuf> {
    let content = buffer.content_as_string();
    if content.trim().is_empty() {
        return None;
    }
    let path = workspace_root.join(".nit").join("legacy_notes.md");
    if path.exists() {
        return Some(path);
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::write(&path, content).is_ok() {
        Some(path)
    } else {
        None
    }
}
