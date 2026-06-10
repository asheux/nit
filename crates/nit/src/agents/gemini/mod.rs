mod classify;
mod js_parser;

use std::collections::HashMap;
use std::fs;

use crate::agents::discover::{
    find_executable_in_path, probe_models_from_cli, DEFAULT_MODEL_LIST_ARG_SETS,
};

use classify::{classify_gemini_model, record_if_better, ModelCandidate};

pub(crate) use js_parser::parse_gemini_models_from_source;

pub(in crate::agents) fn probe_gemini_models() -> (Vec<String>, Option<String>) {
    // Gemini's CLI also recognizes `--models`, which neither codex nor claude
    // accept — append it after the shared set rather than redefining the list.
    let mut cli_attempts: Vec<&[&str]> = DEFAULT_MODEL_LIST_ARG_SETS.to_vec();
    cli_attempts.push(&["--models"]);

    let (raw_models, probe_error) = probe_models_from_cli("gemini", &cli_attempts);
    let filtered = select_current_gemini_models(raw_models);
    if !filtered.is_empty() {
        return (filtered, None);
    }

    if let Some(installed_models) = discover_models_from_package() {
        let filtered_installed = select_current_gemini_models(installed_models);
        return (filtered_installed, None);
    }

    (filtered, probe_error)
}

pub(crate) fn select_current_gemini_models(raw_models: Vec<String>) -> Vec<String> {
    let mut deduplicated = raw_models;
    deduplicated.sort();
    deduplicated.dedup();

    let mut best_per_family: HashMap<String, ModelCandidate> = HashMap::new();

    for model_id in &deduplicated {
        let Some(classified) = classify_gemini_model(model_id) else {
            continue;
        };
        record_if_better(&mut best_per_family, classified, model_id);
    }

    if best_per_family.is_empty() {
        return deduplicated;
    }

    let mut selected: Vec<String> = best_per_family
        .into_values()
        .map(|winner| winner.full_identifier)
        .collect();
    selected.sort();
    selected
}

fn discover_models_from_package() -> Option<Vec<String>> {
    let gemini_bin = find_executable_in_path("gemini")?;
    let canonical_path = fs::canonicalize(gemini_bin).ok()?;
    let package_dir = canonical_path.parent()?.parent()?;

    let models_js_path =
        package_dir.join("node_modules/@google/gemini-cli-core/dist/src/config/models.js");
    let js_content = fs::read_to_string(models_js_path).ok()?;

    let parsed = parse_gemini_models_from_source(&js_content);
    (!parsed.is_empty()).then_some(parsed)
}
