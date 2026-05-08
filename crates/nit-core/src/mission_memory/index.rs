//! Build / upsert the `MissionMemoryIndex` from the on-disk
//! `<workspace>/.nit/swarm/<mission>/{run,summary,tasks}` corpus.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use super::io::{load_index, save_index};
use super::search::{tokenize, tokens_from_paths};
use super::{IndexedMission, MissionMemoryIndex};

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
