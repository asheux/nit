use super::super::types::StrategySpec;
use std::io::BufRead;
use std::path::Path;

pub(in crate::config) fn load_generated_strategies(
    id: &str,
    source: Option<&str>,
    limit: Option<usize>,
    base_dir: Option<&Path>,
) -> Result<Vec<StrategySpec>, Vec<String>> {
    let mut errors = Vec::new();
    let source = match source {
        Some(path) if !path.trim().is_empty() => path.trim(),
        _ => {
            errors.push(format!(
                "strategy '{id}': generated strategies require a source path"
            ));
            return Err(errors);
        }
    };

    let mut path = std::path::PathBuf::from(source);
    if path.is_relative() {
        if let Some(base) = base_dir {
            path = base.join(path);
        } else if let Ok(cwd) = std::env::current_dir() {
            path = cwd.join(path);
        }
    }

    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(err) => {
            errors.push(format!(
                "strategy '{id}': failed to open generated strategies {}: {err}",
                path.display()
            ));
            return Err(errors);
        }
    };
    let reader = std::io::BufReader::new(file);
    let mut specs = Vec::new();
    for (line_idx, line) in reader.lines().enumerate() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                errors.push(format!(
                    "strategy '{id}': failed reading generated strategies {}: {err}",
                    path.display()
                ));
                break;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<StrategySpec>(trimmed) {
            Ok(mut spec) => {
                if !id.is_empty() {
                    spec.id = format!("{id}::{}", spec.id);
                }
                specs.push(spec);
                if let Some(limit) = limit {
                    if specs.len() >= limit {
                        break;
                    }
                }
            }
            Err(err) => {
                errors.push(format!(
                    "strategy '{id}': failed to parse generated strategies at line {}: {err}",
                    line_idx + 1
                ));
                break;
            }
        }
    }

    if errors.is_empty() {
        Ok(specs)
    } else {
        Err(errors)
    }
}
