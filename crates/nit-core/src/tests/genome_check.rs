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
    let files = ["agents/mod.rs", "games/run.rs"];
    for name in &files {
        let full = base.join(name);
        let text = std::fs::read_to_string(&full).unwrap();
        let report = crate::genome_report::compute_genome_report_fast(&text, Path::new(name));
        for score in &report.encoder_scores {
            assert!(
                score.generations_survived > 0,
                "{} encoder {:?} produced 0 generations on {}",
                name,
                score.encoder,
                full.display(),
            );
        }
    }
}
