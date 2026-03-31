#[test]
fn genome_check_files() {
    use std::path::Path;
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("nit/src");
    let files = [
        "agents/claude.rs",
        "agents/mod.rs",
        "games/mod.rs",
        "games/run.rs",
        "games/sweep.rs",
    ];
    for name in &files {
        let full = base.join(name);
        let text = std::fs::read_to_string(&full).unwrap();
        let report = crate::genome_report::compute_genome_report(&text, Path::new(name));
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
