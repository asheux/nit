use std::path::Path;

// Evaluate-genome tool — injected into every agent prompt so agents know nit
// measures their output automatically. They don't call the tool; the runner
// watches for the marker in case an agent explicitly requests a report.
pub const EVALUATE_GENOME_TOOL_DESCRIPTION: &str = r#"
[nit tool: evaluate_genome]
nit evaluates genome quality automatically in real time as you write files.
You do NOT need to call [evaluate_genome] — nit measures quality externally
after your changes are written to disk. If quality degrades, nit will retry
your turn automatically with specific per-encoder feedback.

Focus on writing good code using the encoder guide and recommendations above.
nit handles the measurement.
[/nit tool]
"#;

// Looks for `[evaluate_genome:<path>]` in agent output and returns a formatted
// genome report when the path resolves to a readable file.
pub fn handle_evaluate_genome_request(workspace_root: &Path, message: &str) -> Option<String> {
    let marker = "[evaluate_genome:";
    let start = message.find(marker)?;
    let rest = &message[start + marker.len()..];
    let end = rest.find(']')?;
    let raw_path = rest[..end].trim();
    if raw_path.is_empty() {
        return None;
    }
    let file_path = if std::path::Path::new(raw_path).is_absolute() {
        std::path::PathBuf::from(raw_path)
    } else {
        workspace_root.join(raw_path)
    };
    let text = std::fs::read_to_string(&file_path).ok()?;
    let report = nit_core::compute_genome_report(&text, &file_path);
    Some(nit_core::format_genome_report(&report))
}
