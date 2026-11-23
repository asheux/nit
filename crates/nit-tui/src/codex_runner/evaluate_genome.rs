use std::path::Path;

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
