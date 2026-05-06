//! On-disk persistence for genome reports under `<workspace>/.nit/genome/`.
//!
//! Filenames are derived from the source path with `/` replaced by `__` so a
//! single directory holds the full crate tree without nested mkdir.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::genome_report::GenomeReport;

fn genome_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".nit").join("genome")
}

fn report_filename(file_path: &Path) -> String {
    format!("{}.json", file_path.to_string_lossy().replace('/', "__"))
}

pub fn persist_genome_report(workspace_root: &Path, report: &GenomeReport) {
    let dir = genome_dir(workspace_root);
    let _ = std::fs::create_dir_all(&dir);
    let filename = report_filename(&report.file_path);
    if let Ok(json) = serde_json::to_string(report) {
        let _ = std::fs::write(dir.join(filename), json);
    }
}

/// Best-effort delete; silent on missing file or I/O error — the caller is
/// invalidating state that's already gone.
pub fn delete_genome_report(workspace_root: &Path, file_path: &Path) {
    let dir = genome_dir(workspace_root);
    let _ = std::fs::remove_file(dir.join(report_filename(file_path)));
}

pub fn load_genome_reports(workspace_root: &Path) -> HashMap<PathBuf, GenomeReport> {
    let mut map = HashMap::new();
    let dir = genome_dir(workspace_root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return map;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(data) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(report) = serde_json::from_str::<GenomeReport>(&data) else {
            continue;
        };
        map.insert(report.file_path.clone(), report);
    }
    map
}
