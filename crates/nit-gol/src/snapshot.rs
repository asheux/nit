use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::{Grid, Rule};

#[derive(Clone, Debug, serde::Serialize)]
pub struct SnapshotMetadata {
    pub timestamp: String,
    pub workspace_root: Option<String>,
    pub file_path: Option<String>,
    pub seed_source: String,
    pub seed_hash: u64,
    pub rule: String,
    pub generation: u64,
    pub alive_count: usize,
    pub period: Option<u32>,
    pub score: Option<f32>,
    pub wrap_mode: String,
    pub tick_ms: u64,
}

#[derive(Clone, Debug)]
pub struct SnapshotPaths {
    pub rle_path: PathBuf,
    pub json_path: PathBuf,
}

pub fn write_snapshot(
    dir: &Path,
    name_base: &str,
    grid: &Grid,
    rule: Rule,
    meta: &SnapshotMetadata,
) -> io::Result<SnapshotPaths> {
    ensure_dir(dir)?;
    let rle_path = dir.join(format!("{name_base}.rle"));
    let json_path = dir.join(format!("{name_base}.json"));
    let rle = encode_rle(grid, rule);
    write_atomic(&rle_path, rle.as_bytes())?;
    let json = serde_json::to_vec_pretty(meta).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    write_atomic(&json_path, &json)?;
    Ok(SnapshotPaths { rle_path, json_path })
}

pub fn encode_rle(grid: &Grid, rule: Rule) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "x = {}, y = {}, rule = {}\n",
        grid.width(),
        grid.height(),
        rule
    ));
    if grid.width() == 0 || grid.height() == 0 {
        out.push('!');
        return out;
    }
    let mut line = String::new();
    for y in 0..grid.height() {
        let mut run_char = if grid.get(0, y) { 'o' } else { 'b' };
        let mut run_len = 1usize;
        for x in 1..grid.width() {
            let cell = if grid.get(x, y) { 'o' } else { 'b' };
            if cell == run_char {
                run_len += 1;
            } else {
                push_run(&mut line, run_len, run_char);
                run_char = cell;
                run_len = 1;
            }
        }
        push_run(&mut line, run_len, run_char);
        if y + 1 < grid.height() {
            line.push('$');
        }
    }
    line.push('!');
    wrap_rle(&mut out, &line);
    out
}

pub fn default_name(rule: Rule, generation: u64, hash: u64) -> String {
    let timestamp = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown-time".into())
        .replace(':', "-");
    format!(
        "{timestamp}__rule-{}__gen-{generation:05}__hash-{hash:08x}",
        rule.to_string().replace('/', "")
    )
}

pub fn now_iso8601() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown-time".into())
}

pub fn prune_oldest(dir: &Path, max_files: usize) -> io::Result<()> {
    if max_files == 0 {
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok()?;
            Some((modified, e.path()))
        })
        .collect();
    if entries.len() <= max_files {
        return Ok(());
    }
    entries.sort_by_key(|(time, _)| *time);
    let remove_count = entries.len().saturating_sub(max_files);
    for (_, path) in entries.into_iter().take(remove_count) {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

fn push_run(line: &mut String, len: usize, ch: char) {
    if len > 1 {
        line.push_str(&len.to_string());
    }
    line.push(ch);
}

fn wrap_rle(out: &mut String, data: &str) {
    let mut count = 0usize;
    for ch in data.chars() {
        out.push(ch);
        count += 1;
        if count >= 70 && ch != '$' && ch != '!' {
            out.push('\n');
            count = 0;
        }
        if ch == '$' {
            out.push('\n');
            count = 0;
        }
    }
}

fn ensure_dir(dir: &Path) -> io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(dir) {
        if meta.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "snapshot dir is a symlink",
            ));
        }
        if meta.is_dir() {
            return Ok(());
        }
    }
    fs::create_dir_all(dir)
}

fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp_path = path.with_extension("tmp");
    let mut file = File::create(&tmp_path)?;
    file.write_all(data)?;
    file.sync_all()?;
    fs::rename(tmp_path, path)?;
    Ok(())
}
