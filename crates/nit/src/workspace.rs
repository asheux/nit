use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use nit_core::{io as core_io, Buffer};
use nit_utils::hashing::stable_hash_bytes;
use nit_utils::paths;

/// Retrieve the HEAD revision of a tracked file via a single `git show` invocation.
/// Returns `None` for untracked files or if git is unavailable.
fn git_head_content(file_path: &Path) -> Option<String> {
    let working_dir = file_path.parent()?;
    // Two-step: resolve repo-relative path, then retrieve HEAD content.
    let tracked_check = Command::new("git")
        .args(["ls-files", "--full-name", "--error-unmatch"])
        .arg(file_path)
        .current_dir(working_dir)
        .output()
        .ok()
        .filter(|result| result.status.success())?;

    let repo_relative = String::from_utf8(tracked_check.stdout).ok()?;
    let head_revision = Command::new("git")
        .args(["show", &format!("HEAD:{}", repo_relative.trim())])
        .current_dir(working_dir)
        .output()
        .ok()
        .filter(|result| result.status.success())?;

    String::from_utf8(head_revision.stdout).ok()
}

fn load_file_buffer(path: &Path, default_name: &str) -> anyhow::Result<Buffer> {
    let content =
        core_io::load_to_string(path).with_context(|| format!("reading {}", path.display()))?;
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

/// Open a target path for the GoL lab.
pub(crate) fn open_target_gol(path: Option<&Path>) -> anyhow::Result<(PathBuf, Buffer)> {
    open_target(path, "untitled", empty_gol_buffer)
}

/// Open a target path for the Games lab, loading `games.toml` or scaffolding a template.
pub(crate) fn open_target_games(path: Option<&Path>) -> anyhow::Result<(PathBuf, Buffer)> {
    open_target(path, "games.toml", games_dir_buffer)
}

/// Shared dispatcher: routes file / dir / missing / nonexistent paths through a
/// common pattern, with `dir_handler` called when the target is a directory.
fn open_target(
    path: Option<&Path>,
    default_name: &str,
    dir_handler: fn(&Path) -> anyhow::Result<(PathBuf, Buffer)>,
) -> anyhow::Result<(PathBuf, Buffer)> {
    match path {
        Some(p) if p.is_file() => {
            let buffer = load_file_buffer(p, default_name)?;
            let root = parent_or_cwd(p)?;
            Ok((root, buffer))
        }
        Some(p) if p.is_dir() => dir_handler(p),
        None => dir_handler(&std::env::current_dir()?),
        Some(missing) => anyhow::bail!("path does not exist: {}", missing.display()),
    }
}

fn empty_gol_buffer(dir: &Path) -> anyhow::Result<(PathBuf, Buffer)> {
    Ok((dir.to_path_buf(), Buffer::empty("untitled", None)))
}

fn games_dir_buffer(dir: &Path) -> anyhow::Result<(PathBuf, Buffer)> {
    let root = dir.to_path_buf();
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
    let candidate = std::env::current_dir()
        .ok()?
        .join("assets/themes/devs.toml");
    candidate.exists().then_some(candidate)
}

pub(crate) fn load_notes(workspace_root: &Path) -> Buffer {
    let Some(notes_path) = hashed_state_path(workspace_root, "notes", "md") else {
        return Buffer::empty("notes", None);
    };
    if !notes_path.exists() {
        return Buffer::empty("notes", Some(notes_path));
    }
    match core_io::load_to_string(&notes_path) {
        Ok(saved_content) => Buffer::from_str("notes", &saved_content, Some(notes_path)),
        Err(_) => Buffer::empty("notes", Some(notes_path)),
    }
}

/// Build a hashed path under the nit state directory: `<state>/<subdir>/<hash>.<ext>`.
fn hashed_state_path(workspace_root: &Path, subdir: &str, ext: &str) -> Option<PathBuf> {
    let base = paths::state_dir().or_else(paths::data_dir)?;
    let dir = base.join(subdir);
    let _ = fs::create_dir_all(&dir);
    let hash = stable_hash_bytes(workspace_root.to_string_lossy().as_bytes());
    Some(dir.join(format!("{hash:016x}.{ext}")))
}

pub(crate) fn export_legacy_notes_snapshot(
    workspace_root: &Path,
    buffer: &Buffer,
) -> Option<PathBuf> {
    let notes_text = buffer.content_as_string();
    if notes_text.trim().is_empty() {
        return None;
    }
    let snapshot_path = workspace_root.join(".nit/legacy_notes.md");
    if snapshot_path.exists() {
        return Some(snapshot_path);
    }
    if let Some(parent_dir) = snapshot_path.parent() {
        let _ = fs::create_dir_all(parent_dir);
    }
    fs::write(&snapshot_path, notes_text)
        .ok()
        .map(|()| snapshot_path)
}
