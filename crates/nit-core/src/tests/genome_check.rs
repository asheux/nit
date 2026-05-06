//! Smoke probe — runs the genome encoders over a couple real source
//! files so the adaptive grid sizing exercises both the small and large
//! input paths without burning CI seconds.

#[test]
fn genome_check_files() {
    use std::path::Path;
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("nit/src");
    // Keep the check list small for fast CI. Two files cover both a small and
    // large input so we exercise adaptive grid sizing without burning seconds.
    let files = ["agents/mod.rs", "games/run.rs"];
    for name in &files {
        let full = base.join(name);
        let text = std::fs::read_to_string(&full).unwrap();
        let report = crate::genome_report::compute_genome_report_fast(&text, Path::new(name));
        eprintln!("\n=== {} (Tier {}) ===", name, report.tier.numeral());
        for score in &report.encoder_scores {
            if matches!(
                score.encoder,
                crate::seed::SeedEncoderId::TokenSpectrum
                    | crate::seed::SeedEncoderId::AstStructure
                    | crate::seed::SeedEncoderId::ComplexityField
                    | crate::seed::SeedEncoderId::Structural
            ) {
                eprintln!(
                    "  {}: gens={}, density={:.2}, components={}",
                    score.encoder.label(),
                    score.generations_survived,
                    score.density,
                    score.components,
                );
            }
        }
    }
}
