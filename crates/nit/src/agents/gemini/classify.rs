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
    let dominated_by_existing =
        family_winners
            .get(classification.family_tag)
            .is_some_and(|incumbent| {
                // Stable releases always beat preview siblings; otherwise prefer the higher
                // dotted version, with ties broken by shorter identifier.
                if incumbent.preview_variant != classification.preview_variant {
                    return classification.preview_variant;
                }
                classification.version_components < incumbent.version_components
                    || (classification.version_components == incumbent.version_components
                        && !prefer_shorter_model_name(full_id, &incumbent.full_identifier))
            });

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

fn parse_dotted_version(raw: &str) -> Option<Vec<u32>> {
    raw.split('.')
        .map(|segment| {
            (!segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()))
                .then(|| segment.parse().ok())
                .flatten()
        })
        .collect()
}
