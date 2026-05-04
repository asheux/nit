use std::collections::HashMap;

use crate::agents::prefer_shorter_model_name;

pub(super) struct ClassifiedModel {
    pub(super) family_tag: &'static str,
    pub(super) preview_variant: bool,
    pub(super) version_components: Vec<u32>,
}

pub(super) struct ModelCandidate {
    preview_variant: bool,
    version_components: Vec<u32>,
    pub(super) full_identifier: String,
}

pub(super) fn classify_gemini_model(raw_identifier: &str) -> Option<ClassifiedModel> {
    let normalized_name = raw_identifier.trim().to_ascii_lowercase();
    let remainder = normalized_name.strip_prefix("gemini-")?;
    let (numeric_portion, descriptor) = remainder.split_once('-')?;
    let parsed_version = parse_dotted_version(numeric_portion)?;

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

pub(super) fn record_if_better(
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

    challenger.version_components > current_best.version_components
        || (challenger.version_components == current_best.version_components
            && prefer_shorter_model_name(challenger_name, &current_best.full_identifier))
}

fn parse_dotted_version(raw: &str) -> Option<Vec<u32>> {
    raw.split('.')
        .map(|segment| {
            (!segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()))
                .then(|| segment.parse().ok())
                .flatten()
        })
        .collect()
}
