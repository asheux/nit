//! On-disk persistence for genome reports under `<workspace>/.nit/genome/`.
//!
//! Filenames are derived from the source path with `/` replaced by `__` so a
//! single directory holds the full crate tree without nested mkdir. This
//! flattening is intentional: the genome cache is a content-addressed lookup,
//! not a mirror of the source tree, so directory traversal stays O(files)
//! rather than O(files + dirs) and individual reports can be deleted by name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::genome_report::GenomeReport;

const GENOME_DIR_NAME: &str = "genome";
const NIT_DIR_NAME: &str = ".nit";
const REPORT_EXTENSION: &str = "json";

pub fn persist_genome_report(workspace_root: &Path, report: &GenomeReport) {
    let dir = genome_dir(workspace_root);
    let _ = std::fs::create_dir_all(&dir);
    let Ok(json) = serde_json::to_string(report) else {
        return;
    };
    let path = dir.join(report_filename(&report.file_path));
    let _ = std::fs::write(path, json);
}

/// Best-effort delete; silent on missing file or I/O error — the caller is
/// invalidating state that's already gone.
pub fn delete_genome_report(workspace_root: &Path, file_path: &Path) {
    let path = genome_dir(workspace_root).join(report_filename(file_path));
    let _ = std::fs::remove_file(path);
}

pub fn load_genome_reports(workspace_root: &Path) -> HashMap<PathBuf, GenomeReport> {
    let dir = genome_dir(workspace_root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|entry| read_report_at(&entry.path()))
        .map(|report| (report.file_path.clone(), report))
        .collect()
}

fn read_report_at(path: &Path) -> Option<GenomeReport> {
    if path.extension().and_then(|e| e.to_str()) != Some(REPORT_EXTENSION) {
        return None;
    }
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<GenomeReport>(&data).ok()
}

fn genome_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(NIT_DIR_NAME).join(GENOME_DIR_NAME)
}

fn report_filename(file_path: &Path) -> String {
    format!(
        "{}.{REPORT_EXTENSION}",
        file_path.to_string_lossy().replace('/', "__")
    )
}
