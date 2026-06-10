use std::collections::HashMap;

use crate::agents::prefer_shorter_model_name;

pub(super) struct ClassifiedModel {
    pub(super) family_tag: String,
    pub(super) preview_variant: bool,
    pub(super) version_components: Vec<u32>,
}

pub(super) struct ModelCandidate {
    preview_variant: bool,
    version_components: Vec<u32>,
    pub(super) full_identifier: String,
}

// Specialized, non-chat Gemini variants we never surface as agent lanes.
const NON_CHAT_MARKERS: &[&str] = &["customtools", "embedding", "tts", "image", "audio", "aqa"];

// Stability / variant qualifiers that follow the family in an id. They are NOT
// part of the family key (so `flash-preview` groups under `flash`), but unknown
// tokens ARE kept as family parts — that's what makes the classifier generic:
// a new `gemini-3.0-ultra` becomes family `ultra` instead of being dropped.
const STABILITY_QUALIFIERS: &[&str] = &[
    "preview",
    "exp",
    "experimental",
    "thinking",
    "latest",
    "nothink",
];

pub(super) fn classify_gemini_model(raw_identifier: &str) -> Option<ClassifiedModel> {
    let normalized_name = raw_identifier.trim().to_ascii_lowercase();
    let remainder = normalized_name.strip_prefix("gemini-")?;
    let (numeric_portion, descriptor) = remainder.split_once('-')?;
    let parsed_version = parse_dotted_version(numeric_portion)?;

    if NON_CHAT_MARKERS.iter().any(|m| descriptor.contains(m)) {
        return None;
    }

    let family_tag = family_from_descriptor(descriptor)?;

    Some(ClassifiedModel {
        family_tag,
        preview_variant: descriptor.contains("preview") || descriptor.contains("exp"),
        version_components: parsed_version,
    })
}

/// Family = the leading alphabetic descriptor tokens, stopping at the first
/// numeric/date segment or stability qualifier. `flash` → `flash`,
/// `flash-lite` → `flash-lite`, `flash-preview-05-20` → `flash`. Generic over
/// family: any new tier classifies by its own name, no allowlist to update.
fn family_from_descriptor(descriptor: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for token in descriptor.split('-') {
        if token.is_empty() {
            continue;
        }
        if token.bytes().all(|b| b.is_ascii_digit()) || STABILITY_QUALIFIERS.contains(&token) {
            break;
        }
        parts.push(token);
    }
    (!parts.is_empty()).then(|| parts.join("-"))
}

pub(super) fn record_if_better(
    family_winners: &mut HashMap<String, ModelCandidate>,
    classification: ClassifiedModel,
    full_id: &str,
) {
    let dominated_by_existing =
        family_winners
            .get(&classification.family_tag)
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
