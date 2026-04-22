use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use nit_core::{io as core_io, Buffer};

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

pub(super) fn load_file_buffer(path: &Path, default_name: &str) -> anyhow::Result<Buffer> {
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

pub(super) fn parent_or_cwd(path: &Path) -> anyhow::Result<PathBuf> {
    match path.parent() {
        Some(p) => Ok(p.to_path_buf()),
        None => Ok(std::env::current_dir()?),
    }
}
