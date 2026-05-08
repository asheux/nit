//! Detect when the structural encoder is a significant outlier and emit
//! targeted recommendations.
//!
//! The structural encoder is AST-aware but whitespace-filtered, so it tends
//! to expose token-role repetition that the other encoders miss. When it
//! lags well behind the AST mean we surface a specific cause (low density,
//! few components, or a generic outlier note) rather than a vague "structure
//! is off" message.

use crate::seed::SeedEncoderId;

use super::{EncoderScore, GenomeRecommendation, RecommendationSeverity};

const OUTLIER_MIN_AST_MEAN: f32 = 50.0;
const OUTLIER_GENERATIONS_RATIO: f32 = 0.3;
const STRUCTURAL_LOW_DENSITY: f32 = 0.10;
const STRUCTURAL_LOW_COMPONENT_COUNT: usize = 5;

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

    let Some(ast_mean) = compute_ast_mean(scores) else {
        return;
    };

    if !is_outlier(structural, ast_mean) {
        return;
    }

    let mut specific = false;
    if structural.density < STRUCTURAL_LOW_DENSITY {
        recs.push(low_density_warning(structural.density));
        specific = true;
    }
    if structural.components < STRUCTURAL_LOW_COMPONENT_COUNT {
        recs.push(few_components_warning(structural.components));
        specific = true;
    }
    if !specific {
        recs.push(general_outlier_warning(
            structural.generations_survived,
            ast_mean,
        ));
    }
}

fn compute_ast_mean(scores: &[EncoderScore]) -> Option<f32> {
    let ast_gens: Vec<u32> = scores
        .iter()
        .filter(|s| s.encoder != SeedEncoderId::Structural)
        .map(|s| s.generations_survived)
        .collect();
    if ast_gens.is_empty() {
        return None;
    }
    Some(ast_gens.iter().sum::<u32>() as f32 / ast_gens.len() as f32)
}

fn is_outlier(structural: &EncoderScore, ast_mean: f32) -> bool {
    ast_mean > OUTLIER_MIN_AST_MEAN
        && (structural.generations_survived as f32) < ast_mean * OUTLIER_GENERATIONS_RATIO
}

fn low_density_warning(density: f32) -> GenomeRecommendation {
    GenomeRecommendation {
        metric: "structural_diversity".into(),
        severity: RecommendationSeverity::Warning,
        message: format!(
            "Structural encoder bottleneck: density {density:.2} is very low. The token-role \
             distribution lacks variety. This usually means code is structurally repetitive. \
             Solve different sub-problems with naturally different approaches rather than \
             repeating the same pattern. Do NOT add comments just for diversity."
        ),
        location: None,
    }
}

fn few_components_warning(components: usize) -> GenomeRecommendation {
    GenomeRecommendation {
        metric: "structural_ngram".into(),
        severity: RecommendationSeverity::Warning,
        message: format!(
            "Structural encoder bottleneck: only {components} connected regions in the GoL grid. \
             The code has too many repeated structural patterns. Functions likely share \
             the same role sequence (e.g., keyword-variable-operator-punctuation). Vary \
             function signatures, error handling styles, and intersperse different node \
             types (closures, trait impls, enums, const items)."
        ),
        location: None,
    }
}

fn general_outlier_warning(generations: u32, ast_mean: f32) -> GenomeRecommendation {
    GenomeRecommendation {
        metric: "structural_general".into(),
        severity: RecommendationSeverity::Warning,
        message: format!(
            "Structural encoder is a severe outlier ({generations} generations vs {ast_mean:.0} AST mean). \
             This encoder measures token-role diversity, AST depth variation, role entropy, \
             and role-pattern uniqueness. The code likely has repeated structural patterns. \
             Write naturally varied code — different sub-problems should produce different \
             shapes. Do NOT add comments or artificial variety to game this encoder."
        ),
        location: None,
    }
}
