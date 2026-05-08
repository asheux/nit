//! Smoke probe — runs the genome encoders over real source files so the
//! adaptive grid sizing exercises both small and large input paths without
//! burning CI seconds.

use std::path::Path;

use crate::genome_report::compute_genome_report_fast;

#[test]
fn genome_check_files() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("nit/src");
    for name in ["agents/mod.rs", "games/run.rs"] {
        let full = base.join(name);
        let text = std::fs::read_to_string(&full).unwrap();
        let report = compute_genome_report_fast(&text, Path::new(name));
        for score in &report.encoder_scores {
            assert!(
                score.generations_survived > 0,
                "{name} encoder {:?} produced 0 generations on {}",
                score.encoder,
                full.display(),
            );
        }
    }
}
