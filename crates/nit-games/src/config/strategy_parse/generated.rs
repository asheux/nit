use super::super::types::StrategySpec;
use std::io::BufRead;
use std::path::{Path, PathBuf};

pub(in crate::config) fn load_generated_strategies(
    id: &str,
    source: Option<&str>,
    limit: Option<usize>,
    base_dir: Option<&Path>,
) -> Result<Vec<StrategySpec>, Vec<String>> {
    let path = match resolve_source_path(id, source, base_dir) {
        Ok(path) => path,
        Err(err) => return Err(vec![err]),
    };
    let file = std::fs::File::open(&path).map_err(|err| {
        vec![format!(
            "strategy '{id}': failed to open generated strategies {}: {err}",
            path.display()
        )]
    })?;
    parse_jsonl_specs(id, std::io::BufReader::new(file), &path, limit)
}

fn resolve_source_path(
    id: &str,
    source: Option<&str>,
    base_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let source = source
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("strategy '{id}': generated strategies require a source path"))?;

    let mut path = PathBuf::from(source);
    if path.is_relative() {
        if let Some(base) = base_dir {
            path = base.join(path);
        } else if let Ok(cwd) = std::env::current_dir() {
            path = cwd.join(path);
        }
    }
    Ok(path)
}

fn parse_jsonl_specs<R: BufRead>(
    id: &str,
    reader: R,
    path: &Path,
    limit: Option<usize>,
) -> Result<Vec<StrategySpec>, Vec<String>> {
    let mut errors = Vec::new();
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
                if matches!(limit, Some(cap) if specs.len() >= cap) {
                    break;
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
