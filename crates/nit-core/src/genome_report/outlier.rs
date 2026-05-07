use crate::seed::SeedEncoderId;

use super::{EncoderScore, GenomeRecommendation, RecommendationSeverity};

// Detect when the structural encoder is a significant outlier and emit
// targeted recommendations (the per-channel weighting lives with the seed
// encoder definition).

const OUTLIER_MIN_AST_MEAN: f32 = 50.0;
const OUTLIER_GENERATIONS_RATIO: f32 = 0.3;
const STRUCTURAL_LOW_DENSITY: f32 = 0.10;
const STRUCTURAL_LOW_COMPONENT_COUNT: usize = 5;

const STRUCTURAL_DENSITY_WARN: &str =
    "Structural encoder bottleneck: density {density:.2} is very low. The token-role \
     distribution lacks variety. This usually means code is structurally repetitive. \
     Solve different sub-problems with naturally different approaches rather than \
     repeating the same pattern. Do NOT add comments just for diversity.";

const STRUCTURAL_NGRAM_WARN: &str =
    "Structural encoder bottleneck: only {components} connected regions in the GoL grid. \
     The code has too many repeated structural patterns. Functions likely share \
     the same role sequence (e.g., keyword-variable-operator-punctuation). Vary \
     function signatures, error handling styles, and intersperse different node \
     types (closures, trait impls, enums, const items).";

const STRUCTURAL_GENERAL_WARN: &str =
    "Structural encoder is a severe outlier ({generations} generations vs {mean:.0} AST mean). \
     This encoder measures token-role diversity, AST depth variation, role entropy, \
     and role-pattern uniqueness. The code likely has repeated structural patterns. \
     Write naturally varied code — different sub-problems should produce different \
     shapes. Do NOT add comments or artificial variety to game this encoder.";

pub(super) fn analyze_structural_outlier(
    _text: &str,
    scores: &[EncoderScore],
    recs: &mut Vec<GenomeRecommendation>,
) {
    let Some(structural) = scores
        .iter()
        .find(|s| s.encoder == SeedEncoderId::Structural)
    else {
        return;
    };

    let ast_gens: Vec<u32> = scores
        .iter()
        .filter(|s| s.encoder != SeedEncoderId::Structural)
        .map(|s| s.generations_survived)
        .collect();
    if ast_gens.is_empty() {
        return;
    }
    let ast_mean = ast_gens.iter().sum::<u32>() as f32 / ast_gens.len() as f32;

    let is_outlier = ast_mean > OUTLIER_MIN_AST_MEAN
        && (structural.generations_survived as f32) < ast_mean * OUTLIER_GENERATIONS_RATIO;
    if !is_outlier {
        return;
    }

    let mut specific = false;

    if structural.density < STRUCTURAL_LOW_DENSITY {
        recs.push(GenomeRecommendation {
            metric: "structural_diversity".into(),
            severity: RecommendationSeverity::Warning,
            message: STRUCTURAL_DENSITY_WARN.replace(
                "{density:.2}",
                &format!("{density:.2}", density = structural.density),
            ),
            location: None,
        });
        specific = true;
    }

    if structural.components < STRUCTURAL_LOW_COMPONENT_COUNT {
        recs.push(GenomeRecommendation {
            metric: "structural_ngram".into(),
            severity: RecommendationSeverity::Warning,
            message: STRUCTURAL_NGRAM_WARN
                .replace("{components}", &structural.components.to_string()),
            location: None,
        });
        specific = true;
    }

    if !specific {
        recs.push(GenomeRecommendation {
            metric: "structural_general".into(),
            severity: RecommendationSeverity::Warning,
            message: STRUCTURAL_GENERAL_WARN
                .replace(
                    "{generations}",
                    &structural.generations_survived.to_string(),
                )
                .replace("{mean:.0}", &format!("{ast_mean:.0}")),
            location: None,
        });
    }
}
