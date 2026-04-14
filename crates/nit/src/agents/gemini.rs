use std::collections::HashMap;
use std::fs;

use super::discover::{find_executable_in_path, probe_models_from_cli};

pub(super) fn probe_gemini_models() -> (Vec<String>, Option<String>) {
    let cli_attempts: &[&[&str]] = &[
        &["models", "--json"],
        &["models"],
        &["list-models"],
        &["--list-models"],
        &["--models"],
    ];

    let (raw_models, probe_error) = probe_models_from_cli("gemini", cli_attempts);
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

pub(crate) fn parse_gemini_models_from_source(js_source: &str) -> Vec<String> {
    let const_bindings = collect_js_const_bindings(js_source);
    let set_member_tokens = extract_valid_models_set_members(js_source);

    let mut resolved: Vec<String> = set_member_tokens
        .into_iter()
        .filter_map(|token| resolve_set_member(token, &const_bindings))
        .collect();
    resolved.sort();
    resolved.dedup();
    resolved
}

pub(crate) fn select_current_gemini_models(raw_models: Vec<String>) -> Vec<String> {
    let mut deduplicated = raw_models;
    deduplicated.sort();
    deduplicated.dedup();

    let mut best_per_family: HashMap<&'static str, ModelCandidate> = HashMap::new();

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

struct ClassifiedModel {
    family_tag: &'static str,
    preview_variant: bool,
    version_components: Vec<u32>,
}

struct ModelCandidate {
    preview_variant: bool,
    version_components: Vec<u32>,
    full_identifier: String,
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

fn resolve_set_member(token: &str, bindings: &HashMap<String, String>) -> Option<String> {
    strip_single_quotes(token)
        .map(|v| v.to_string())
        .or_else(|| bindings.get(token).cloned())
}

fn collect_js_const_bindings(js_source: &str) -> HashMap<String, String> {
    js_source.lines().filter_map(parse_js_const_line).collect()
}

fn parse_js_const_line(line: &str) -> Option<(String, String)> {
    let after_export = line.trim().strip_prefix("export const ")?;
    let (binding_name, rhs) = after_export.split_once('=')?;
    let cleaned_rhs = rhs.trim().trim_end_matches(';').trim();
    let unquoted = strip_single_quotes(cleaned_rhs)?;
    Some((binding_name.trim().to_string(), unquoted.to_string()))
}

fn extract_valid_models_set_members(js_source: &str) -> Vec<&str> {
    let set_constructor_prefix = "export const VALID_GEMINI_MODELS = new Set([";

    let Some(prefix_offset) = js_source.find(set_constructor_prefix) else {
        return Vec::new();
    };

    let inner_content = &js_source[prefix_offset + set_constructor_prefix.len()..];

    let Some(terminator_offset) = inner_content.find("]);") else {
        return Vec::new();
    };

    inner_content[..terminator_offset]
        .split(',')
        .map(|element| element.trim())
        .filter(|element| !element.is_empty())
        .collect()
}

fn strip_single_quotes(text: &str) -> Option<&str> {
    let inner = text.trim().strip_prefix('\'')?.strip_suffix('\'')?;
    (!inner.is_empty()).then_some(inner)
}

fn classify_gemini_model(raw_identifier: &str) -> Option<ClassifiedModel> {
    let normalized_name = raw_identifier.trim().to_ascii_lowercase();
    let remainder = normalized_name.strip_prefix("gemini-")?;
    let (numeric_portion, descriptor) = remainder.split_once('-')?;
    let parsed_version = parse_dotted_version(numeric_portion)?;

    // Exclude special-purpose model variants from family comparison.
    if descriptor.contains("customtools") || descriptor.contains("embedding") {
        return None;
    }

    let family_tag = if descriptor.contains("flash-lite") {
        "flash-lite"
    } else if descriptor.contains("flash") {
        "flash"
    } else if descriptor.contains("pro") {
        "pro"
    } else {
        return None;
    };

    Some(ClassifiedModel {
        family_tag,
        preview_variant: descriptor.contains("preview"),
        version_components: parsed_version,
    })
}

fn record_if_better(
    family_winners: &mut HashMap<&'static str, ModelCandidate>,
    classification: ClassifiedModel,
    full_id: &str,
) {
    let dominated_by_existing = family_winners
        .get(classification.family_tag)
        .is_some_and(|current_best| !beats_incumbent(current_best, &classification, full_id));

    if dominated_by_existing {
        return;
    }

    family_winners.insert(
        classification.family_tag,
        ModelCandidate {
            preview_variant: classification.preview_variant,
            version_components: classification.version_components,
            full_identifier: full_id.to_string(),
        },
    );
}

fn beats_incumbent(
    current_best: &ModelCandidate,
    challenger: &ClassifiedModel,
    challenger_name: &str,
) -> bool {
    if current_best.preview_variant != challenger.preview_variant {
        return !challenger.preview_variant;
    }

    // Same stability tier: higher version wins, then shorter name.
    challenger.version_components > current_best.version_components
        || (challenger.version_components == current_best.version_components
            && super::prefer_shorter_model_name(challenger_name, &current_best.full_identifier))
}

fn parse_dotted_version(raw: &str) -> Option<Vec<u32>> {
    raw.split('.').map(parse_version_segment).collect()
}

fn parse_version_segment(s: &str) -> Option<u32> {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        s.parse().ok()
    } else {
        None
    }
}
