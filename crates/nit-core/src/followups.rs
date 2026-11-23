//! Receipts for operator notes addressed to individual swarm tasks.
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct Source {
    pub path: String,
    pub revision: Option<String>,
    pub hunk: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Submitted,
    Delivered,
    Failed,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Submitted => "submitted",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Receipt {
    pub id: String,
    pub run_id: String,
    pub mission_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub text: String,
    pub status: Status,
    pub detail: Option<String>,
    pub source: Option<Source>,
}

pub fn directory(workspace: &Path, mission_id: &str) -> PathBuf {
    workspace
        .join(".nit")
        .join("swarm")
        .join(mission_id)
        .join("followups")
}

pub fn ensure_directory(workspace: &Path, components: &[&str]) -> io::Result<PathBuf> {
    let mut dir = workspace.to_path_buf();
    for &component in components {
        if !nit_utils::paths::is_safe_leaf_name(component) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid workspace directory name",
            ));
        }
        dir.push(component);
        match fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e)
                if e.kind() == io::ErrorKind::AlreadyExists
                    && fs::symlink_metadata(&dir)?.is_dir() => {}
            Err(e) => return Err(e),
        }
    }
    Ok(dir)
}

fn entries(dir: &Path) -> io::Result<Option<fs::ReadDir>> {
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.is_dir() => fs::read_dir(dir).map(Some),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

// Search filenames before opening receipts. Selecting a note reads just its file.
fn paths(workspace: &Path, prefix: &str) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    match fs::symlink_metadata(workspace.join(".nit")) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Ok(files),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(files),
        Err(e) => return Err(e),
    }
    let root = workspace.join(".nit/swarm");
    for mission in entries(&root)?.into_iter().flatten() {
        let mission = mission?;
        if !mission.file_type()?.is_dir() {
            continue;
        }
        for entry in entries(&mission.path().join("followups"))?
            .into_iter()
            .flatten()
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|s| s == "json")
                && path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with(prefix))
                && entry.file_type()?.is_file()
            {
                files.push(path);
            }
        }
    }
    files.sort_unstable_by(|a, b| b.file_name().cmp(&a.file_name()));
    Ok(files)
}

fn read(path: &Path) -> io::Result<Receipt> {
    let file = fs::File::open(path)?;
    let receipt: Receipt = serde_json::from_reader(io::BufReader::new(file)).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {e}", path.display()),
        )
    })?;
    if path.file_stem().and_then(|id| id.to_str()) != Some(&receipt.id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "receipt ID differs from filename",
        ));
    }
    Ok(receipt)
}

pub fn find(workspace: &Path, id: &str) -> io::Result<Receipt> {
    if id.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "enter a receipt ID",
        ));
    }
    let matches = paths(workspace, id)?;
    let exact = matches
        .iter()
        .filter(|p| p.file_stem().and_then(|s| s.to_str()) == Some(id))
        .collect::<Vec<_>>();
    match (exact.as_slice(), matches.as_slice()) {
        ([path], _) => read(path),
        ([], [path]) => read(path),
        (_, []) => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no matching follow-up in this workspace",
        )),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "multiple follow-ups match; enter the full receipt ID",
        )),
    }
}

fn summary(note: &Receipt) -> String {
    format!(
        "{}  {}  {} / {} → {}",
        note.id,
        note.status.label(),
        note.mission_id,
        note.task_id,
        note.agent_id
    )
}

pub fn describe(note: &Receipt) -> String {
    let mut out = summary(note);
    if let Some(source) = &note.source {
        let _ = write!(out, "\n{}", source.path);
        if let Some(revision) = &source.revision {
            let _ = write!(out, " · {revision}");
        }
        if let Some(hunk) = &source.hunk {
            let _ = write!(out, " · {hunk}");
        }
    }
    if let Some(detail) = &note.detail {
        let _ = write!(out, "\n{detail}");
    }
    let _ = write!(out, "\n\n{}", note.text);
    out
}

pub fn list(workspace: &Path, limit: Option<usize>) -> io::Result<String> {
    let paths = paths(workspace, "")?;
    if paths.is_empty() {
        return Ok("No saved follow-ups in this workspace.".into());
    }
    let count = paths.len();
    let displayed = limit.unwrap_or(count).min(count);
    let mut rows = Vec::with_capacity(displayed + 1);
    for path in paths.into_iter().take(displayed) {
        rows.push(match read(&path) {
            Ok(note) => summary(&note),
            Err(e) => format!("Could not read {}: {e}", path.display()),
        });
    }
    if displayed < count {
        rows.push(format!(
            "Showing {displayed} of {count}. Run nit followups to list all."
        ));
    }
    Ok(rows.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archived_notes_can_be_read_after_restart_and_ambiguous_prefixes_are_rejected() {
        let root = crate::test_helpers::temp_dir("operator-receipts");
        for (mission, id) in [("mis-001", "aa01"), ("mis-002", "aa10")] {
            let dir = directory(&root, mission);
            fs::create_dir_all(&dir).unwrap();
            let note = Receipt {
                id: id.into(),
                run_id: format!("run-{mission}"),
                mission_id: mission.into(),
                agent_id: "worker".into(),
                task_id: "check".into(),
                text: "keep my note\nexactly".into(),
                status: Status::Submitted,
                detail: None,
                source: None,
            };
            fs::write(
                dir.join(format!("{id}.json")),
                serde_json::to_vec(&note).unwrap(),
            )
            .unwrap();
        }
        let found = find(&root, "aa0").unwrap();
        assert_eq!(found.text, "keep my note\nexactly");
        assert_eq!(found.mission_id, "mis-001");
        assert_eq!(
            find(&root, "aa").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(list(&root, Some(1)).unwrap().contains("Showing 1 of 2"));
        fs::write(directory(&root, "mis-002").join("zz.json"), b"oops").unwrap();
        let listing = list(&root, None).unwrap();
        assert!(listing.contains("Could not read"));
        assert!(listing.contains("mis-001 / check"));
        fs::remove_dir_all(root).unwrap();
    }
}
