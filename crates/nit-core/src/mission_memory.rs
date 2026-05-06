//! Cross-mission structural memory.
//!
//! Indexes mission metadata + task summaries from `.nit/swarm/` and
//! retrieves similar past missions at planner time. Pure keyword-based
//! retrieval — no embeddings, no new deps.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexedMission {
    pub mission_id: String,
    pub title: String,
    pub template: String,
    pub status: String,
    pub updated_at: String,
    pub task_ids: Vec<String>,
    pub task_titles: Vec<String>,
    pub task_summaries: Vec<String>,
    pub files_touched: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MissionMemoryIndex {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub missions: Vec<IndexedMission>,
}

fn default_version() -> u32 {
    1
}

#[derive(Clone, Debug)]
pub struct MissionHit {
    pub mission: IndexedMission,
    pub score: f32,
}

pub(crate) const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "of", "in", "to", "for", "with", "on", "at", "by",
    "from", "as", "is", "are", "was", "were", "be", "been", "this", "that", "these", "those", "it",
    "its", "not", "no", "i", "you", "we", "they", "he", "she", "do", "does", "did", "have", "has",
    "had", "will", "would", "should", "could", "can", "may", "might", "must", "so", "if", "then",
    "than", "also", "only", "just", "like", "into", "out", "up", "down", "over", "under", "off",
    "per", "via",
];

pub(crate) fn tokenize(text: &str) -> Vec<String> {
    // Split on Unicode non-word-ish boundaries (keep `_` and `-`), then
    // for each raw token also emit snake_case sub-tokens. Lowercase, drop
    // stopwords, require len >= 2.
    let lower = text.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    for raw in lower.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-') {
        if raw.is_empty() {
            continue;
        }
        // Collect pieces: the raw token and its snake_case sub-tokens.
        let mut pieces: Vec<String> = Vec::new();
        pieces.push(raw.to_string());
        let sub: Vec<&str> = raw.split('_').collect();
        if sub.len() > 1 {
            for s in &sub {
                pieces.push((*s).to_string());
            }
        }
        for piece in pieces {
            if piece.len() < 2 {
                continue;
            }
            if STOPWORDS.contains(&piece.as_str()) {
                continue;
            }
            out.push(piece);
        }
    }
    out
}

fn tokens_from_paths(paths: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for p in paths {
        for part in p.split(['/', '\\', '.']) {
            if part.is_empty() {
                continue;
            }
            for tok in tokenize(part) {
                out.push(tok);
            }
            // Keep the full part too when non-trivial (e.g. "nit-gol").
            let lower = part.to_lowercase();
            if lower.len() >= 2 && !STOPWORDS.contains(&lower.as_str()) {
                out.push(lower);
            }
        }
    }
    out
}

/// Public wrapper around the internal path tokenizer, used at planner call
/// sites to derive scope-file tokens without exposing `pub(crate)` internals.
pub fn path_tokens(paths: &[String]) -> Vec<String> {
    tokens_from_paths(paths)
}

fn index_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".nit")
        .join("memory")
        .join("index.json")
}

pub fn save_index(workspace_root: &Path, index: &MissionMemoryIndex) -> io::Result<()> {
    let path = index_path(workspace_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(index).map_err(io::Error::other)?;
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Tolerant — returns `Default` on missing or corrupt file.
pub fn load_index(workspace_root: &Path) -> MissionMemoryIndex {
    let path = index_path(workspace_root);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => return MissionMemoryIndex::default(),
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn build_index(workspace_root: &Path) -> MissionMemoryIndex {
    let swarm_dir = workspace_root.join(".nit").join("swarm");
    let mut missions: Vec<IndexedMission> = Vec::new();
    let entries = match fs::read_dir(&swarm_dir) {
        Ok(e) => e,
        Err(_) => {
            return MissionMemoryIndex {
                version: 1,
                missions: vec![],
            }
        }
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let mission_id = entry.file_name().to_string_lossy().to_string();
        if !mission_id.starts_with("mis-") {
            continue;
        }
        if let Some(m) = index_one_mission(&entry.path(), &mission_id) {
            missions.push(m);
        }
    }
    missions.sort_by(|a, b| a.mission_id.cmp(&b.mission_id));
    MissionMemoryIndex {
        version: 1,
        missions,
    }
}

fn index_one_mission(dir: &Path, mission_id: &str) -> Option<IndexedMission> {
    let mut m = IndexedMission {
        mission_id: mission_id.to_string(),
        ..Default::default()
    };

    if let Some(run) = read_json(&dir.join("run.json")) {
        merge_run_json(&run, &mut m);
    }
    if let Some(summary) = read_json(&dir.join("summary.json")) {
        merge_summary_json(&summary, &mut m);
    }
    walk_tasks_dir(&dir.join("tasks"), &mut m);

    if m.title.is_empty() && m.task_summaries.is_empty() && m.files_touched.is_empty() {
        return None;
    }

    m.files_touched.sort();
    m.files_touched.dedup();
    m.tags = build_tag_set(&m);

    Some(m)
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

fn merge_run_json(run: &serde_json::Value, m: &mut IndexedMission) {
    let str_at = |key: &str| run.get(key).and_then(|v| v.as_str());
    if let Some(t) = str_at("title") {
        m.title = t.to_string();
    }
    if let Some(t) = str_at("template") {
        m.template = t.to_string();
    }
    if let Some(t) = str_at("status") {
        m.status = t.to_string();
    }
    if let Some(t) = str_at("updated_at") {
        m.updated_at = t.to_string();
    }
    if let Some(arr) = run.get("tasks").and_then(|v| v.as_array()) {
        for t in arr {
            if let Some(id) = t.get("id").and_then(|v| v.as_str()) {
                m.task_ids.push(id.to_string());
            }
            if let Some(title) = t.get("title").and_then(|v| v.as_str()) {
                m.task_titles.push(title.to_string());
            }
        }
    }
}

fn merge_summary_json(summary: &serde_json::Value, m: &mut IndexedMission) {
    let Some(arr) = summary.get("summaries").and_then(|v| v.as_array()) else {
        return;
    };
    for t in arr {
        if let Some(text) = t.get("summary").and_then(|v| v.as_str()) {
            m.task_summaries.push(text.to_string());
        }
    }
}

fn walk_tasks_dir(tasks_dir: &Path, m: &mut IndexedMission) {
    let Ok(entries) = fs::read_dir(tasks_dir) else {
        return;
    };
    for tentry in entries.flatten() {
        let Some(v) = read_json(&tentry.path().join("artifacts.json")) else {
            continue;
        };
        let Some(files) = v.get("files").and_then(|v| v.as_array()) else {
            continue;
        };
        for f in files {
            if let Some(p) = f.get("path").and_then(|v| v.as_str()) {
                m.files_touched.push(p.to_string());
            }
        }
    }
}

fn build_tag_set(m: &IndexedMission) -> Vec<String> {
    let mut tag_set: HashSet<String> = HashSet::new();
    for t in tokenize(&m.title) {
        tag_set.insert(t);
    }
    for s in m.task_titles.iter().chain(m.task_summaries.iter()) {
        for t in tokenize(s) {
            tag_set.insert(t);
        }
    }
    for t in tokens_from_paths(&m.files_touched) {
        tag_set.insert(t);
    }
    let mut tags: Vec<String> = tag_set.into_iter().collect();
    tags.sort();
    tags
}

pub fn load_or_build(workspace_root: &Path) -> MissionMemoryIndex {
    let existing = load_index(workspace_root);
    if !existing.missions.is_empty() {
        return existing;
    }
    let built = build_index(workspace_root);
    let _ = save_index(workspace_root, &built);
    built
}

pub fn upsert_mission(workspace_root: &Path, mission_id: &str) -> io::Result<MissionMemoryIndex> {
    let mut index = load_or_build(workspace_root);
    let dir = workspace_root.join(".nit").join("swarm").join(mission_id);
    if let Some(new_entry) = index_one_mission(&dir, mission_id) {
        index.missions.retain(|m| m.mission_id != mission_id);
        index.missions.push(new_entry);
        index
            .missions
            .sort_by(|a, b| a.mission_id.cmp(&b.mission_id));
    }
    save_index(workspace_root, &index)?;
    Ok(index)
}

/// Smoothed inverse document frequency (IDF): `ln((N + 1) / (df + 1)) + 1`.
/// Smoothing avoids `log(0)`/`log(inf)` singularities when `df` is 0 or
/// matches the corpus size. The `+ 1` offset keeps weights strictly
/// positive so matches always contribute.
pub(crate) fn idf_weight(df: usize, total: usize) -> f32 {
    if total == 0 || df == 0 {
        return 1.0;
    }
    ((total as f32 + 1.0) / (df as f32 + 1.0)).ln() + 1.0
}

/// Count, for each unique term across all mission `tags`, how many
/// missions contain it. Lazy per-query work — O(N * avg_tags) which is
/// negligible at typical corpus sizes.
fn document_frequencies(missions: &[IndexedMission]) -> HashMap<String, usize> {
    let mut df: HashMap<String, usize> = HashMap::new();
    for m in missions {
        let seen: HashSet<&String> = m.tags.iter().collect();
        for t in seen {
            *df.entry(t.clone()).or_insert(0) += 1;
        }
    }
    df
}

pub fn retrieve_similar(
    index: &MissionMemoryIndex,
    query: &str,
    scope_file_tokens: &[String],
    exclude: &[&str],
    k: usize,
) -> Vec<MissionHit> {
    let query_terms: HashSet<String> = tokenize(query)
        .into_iter()
        .chain(scope_file_tokens.iter().cloned())
        .collect();
    if query_terms.is_empty() {
        return Vec::new();
    }
    let query_path_stems: HashSet<String> = scope_file_tokens.iter().cloned().collect();

    // IDF-weighted Jaccard: rare terms count more than common ones. df is
    // computed lazily per query over the full corpus (not just the filtered
    // subset) so weights are stable across calls with different excludes.
    let total = index.missions.len();
    let df = document_frequencies(&index.missions);

    let mut hits: Vec<MissionHit> = index
        .missions
        .iter()
        .filter(|m| !exclude.contains(&m.mission_id.as_str()))
        .filter_map(|m| {
            let mission_terms: HashSet<String> = m.tags.iter().cloned().collect();
            if mission_terms.is_empty() {
                return None;
            }
            let overlap_terms: Vec<&String> = query_terms.intersection(&mission_terms).collect();
            if overlap_terms.is_empty() {
                return None;
            }
            let weighted_overlap: f32 = overlap_terms
                .iter()
                .map(|t| idf_weight(*df.get(*t).unwrap_or(&0), total))
                .sum();
            let weighted_union: f32 = query_terms
                .union(&mission_terms)
                .map(|t| idf_weight(*df.get(t).unwrap_or(&0), total))
                .sum();
            let jaccard = if weighted_union > 0.0 {
                weighted_overlap / weighted_union
            } else {
                0.0
            };

            let title_terms: HashSet<String> = tokenize(&m.title).into_iter().collect();
            let title_overlap = query_terms.intersection(&title_terms).count() as f32;
            let title_boost = (title_overlap * 0.1).min(0.3);

            let mission_path_stems: HashSet<String> =
                tokens_from_paths(&m.files_touched).into_iter().collect();
            let path_overlap = query_path_stems.intersection(&mission_path_stems).count() as f32;
            let path_bonus = (path_overlap * 0.05).min(0.2);

            let score = jaccard + title_boost + path_bonus;
            Some(MissionHit {
                mission: m.clone(),
                score,
            })
        })
        .collect();

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.mission.updated_at.cmp(&a.mission.updated_at))
    });
    hits.truncate(k);
    hits
}

#[cfg(test)]
#[path = "tests/mission_memory.rs"]
mod tests;
