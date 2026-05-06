use crate::seed::SeedEncoderId;

use super::{EncoderScore, GenomeRecommendation, RecommendationSeverity};

/// Detect when the structural encoder is a significant outlier and generate
/// targeted recommendations based on the four token-role channels:
/// role diversity (35%), AST depth (25%), role entropy (20%), role n-gram (20%).
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

    let is_outlier = ast_mean > 50.0 && (structural.generations_survived as f32) < ast_mean * 0.3;
    if !is_outlier {
        return;
    }

    // Low scores on the structural encoder indicate few distinct roles per
    // region, flat AST depth, low role entropy, or repeated role n-gram
    // patterns.
    let mut specific = false;

    if structural.density < 0.10 {
        recs.push(GenomeRecommendation {
            metric: "structural_diversity".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Structural encoder bottleneck: density {:.2} is very low. The token-role \
                 distribution lacks variety. This usually means code is structurally repetitive. \
                 Solve different sub-problems with naturally different approaches rather than \
                 repeating the same pattern. Do NOT add comments just for diversity.",
                structural.density,
            ),
            location: None,
        });
        specific = true;
    }

    // Low components → the GoL seed is too uniform; role patterns repeat.
    if structural.components < 5 {
        recs.push(GenomeRecommendation {
            metric: "structural_ngram".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Structural encoder bottleneck: only {} connected regions in the GoL grid. \
                 The code has too many repeated structural patterns. Functions likely share \
                 the same role sequence (e.g., keyword-variable-operator-punctuation). Vary \
                 function signatures, error handling styles, and intersperse different node \
                 types (closures, trait impls, enums, const items).",
                structural.components,
            ),
            location: None,
        });
        specific = true;
    }

    if !specific {
        recs.push(GenomeRecommendation {
            metric: "structural_general".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Structural encoder is a severe outlier ({} generations vs {:.0} AST mean). \
                 This encoder measures token-role diversity, AST depth variation, role entropy, \
                 and role-pattern uniqueness. The code likely has repeated structural patterns. \
                 Write naturally varied code — different sub-problems should produce different \
                 shapes. Do NOT add comments or artificial variety to game this encoder.",
                structural.generations_survived, ast_mean,
            ),
            location: None,
        });
    }
}
