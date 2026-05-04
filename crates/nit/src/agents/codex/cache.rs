use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;

#[derive(Deserialize)]
struct CodexModelsCache {
    models: Vec<CodexModelEntry>,
}

#[derive(Deserialize)]
pub(super) struct CodexModelEntry {
    pub(super) slug: String,
    #[serde(default)]
    pub(super) display_name: Option<String>,
    #[serde(default)]
    pub(super) description: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    priority: Option<i64>,
    #[serde(default)]
    pub(super) context_window: Option<u32>,
    #[serde(default)]
    pub(super) effective_context_window_percent: Option<u8>,
    #[serde(default)]
    pub(super) default_reasoning_level: Option<String>,
    #[serde(default)]
    pub(super) supported_reasoning_levels: Option<Vec<CodexReasoningLevel>>,
}

#[derive(Deserialize)]
pub(super) struct CodexReasoningLevel {
    pub(super) effort: String,
}

pub(super) fn read_and_sort_entries() -> anyhow::Result<(PathBuf, Vec<CodexModelEntry>)> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let path = PathBuf::from(home).join(".codex").join("models_cache.json");
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let cache: CodexModelsCache =
        serde_json::from_str(&raw).context("parse ~/.codex/models_cache.json")?;

    let mut entries: Vec<_> = cache
        .models
        .into_iter()
        .filter(|m| m.visibility.as_deref().unwrap_or("list") == "list")
        .collect();
    entries.sort_by(|a, b| {
        let ap = a.priority.unwrap_or(i64::MAX);
        let bp = b.priority.unwrap_or(i64::MAX);
        ap.cmp(&bp).then_with(|| a.slug.cmp(&b.slug))
    });
    Ok((path, entries))
}
