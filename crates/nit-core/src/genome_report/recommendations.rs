use std::path::Path;

use super::outlier::analyze_structural_outlier;
use super::source_scan::ts_parse;
use super::{EncoderScore, GenomeRecommendation};

mod density;
mod entropy;
mod function;
mod nesting;
mod thresholds;

pub fn generate_recommendations(
    text: &str,
    file_path: &Path,
    scores: &[EncoderScore],
) -> Vec<GenomeRecommendation> {
    let mut recs = Vec::new();

    density::analyze(scores, &mut recs);

    // Structural encoder is the most common bottleneck. It operates at the raw
    // byte level; detect when it's an outlier and provide specific guidance.
    analyze_structural_outlier(text, scores, &mut recs);

    let Some(tree) = ts_parse(text, file_path) else {
        return recs;
    };
    let lines: Vec<&str> = text.lines().collect();
    let root = tree.root_node();

    function::walk_top_level(text, &lines, &root, &mut recs);
    nesting::analyze(text, &root, &mut recs);
    entropy::analyze(text, &lines, &root, &mut recs);

    recs
}
