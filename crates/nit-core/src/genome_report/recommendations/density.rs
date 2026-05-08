use crate::genome_report::{EncoderScore, GenomeRecommendation, RecommendationSeverity};
use crate::seed::SeedEncoderId;

use super::thresholds::{DENSITY_LOW, MONOLITHIC_COMPONENTS};

pub(super) fn analyze(scores: &[EncoderScore], recs: &mut Vec<GenomeRecommendation>) {
    emit_low_density(scores, recs);
    emit_monolithic_ast(scores, recs);
}

fn emit_low_density(scores: &[EncoderScore], recs: &mut Vec<GenomeRecommendation>) {
    for score in scores {
        let is_ast = matches!(
            score.encoder,
            SeedEncoderId::TokenSpectrum
                | SeedEncoderId::AstStructure
                | SeedEncoderId::ComplexityField,
        );
        if !is_ast || score.density >= DENSITY_LOW {
            continue;
        }
        recs.push(GenomeRecommendation {
            metric: "density".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "{} density is {:.2}. The token-role distribution lacks variety. \
                 Mix different token types: keywords, operators, identifiers, types, and literals. \
                 Break up uniform code blocks with varied function shapes.",
                score.encoder.label(),
                score.density,
            ),
            location: None,
        });
    }
}

fn emit_monolithic_ast(scores: &[EncoderScore], recs: &mut Vec<GenomeRecommendation>) {
    let Some(ast) = scores
        .iter()
        .find(|s| s.encoder == SeedEncoderId::AstStructure)
    else {
        return;
    };
    if ast.components >= MONOLITHIC_COMPONENTS {
        return;
    }
    recs.push(GenomeRecommendation {
        metric: "components".into(),
        severity: RecommendationSeverity::Warning,
        message: format!(
            "AST Structure shows {} components. The code is monolithic. \
             Consider splitting into multiple functions or modules with clear boundaries.",
            ast.components,
        ),
        location: None,
    });
}
