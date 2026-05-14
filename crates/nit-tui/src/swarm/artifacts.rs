use std::collections::HashSet;

use super::{
    extract_json_code_blocks, sanitize_for_filename, SwarmArtifactCommand, SwarmArtifactDiff,
    SwarmArtifactFile, SwarmArtifactFinding, SwarmArtifactRisk, SwarmRun, SwarmTask,
    SwarmTaskArtifacts,
};

pub(super) fn dependency_payload_text(run: &SwarmRun, task: &SwarmTask) -> String {
    if let Some(summary) = task_artifacts_summary_for_prompt(task, &run.mission_id) {
        return summary;
    }
    task.output
        .as_deref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "(no output)".into())
}

pub(super) fn dependency_payload_text_full(task: &SwarmTask) -> String {
    task.output
        .as_deref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "(no output)".into())
}

pub(super) fn task_artifacts_summary_for_prompt(
    task: &SwarmTask,
    mission_id: &str,
) -> Option<String> {
    let artifacts = task.parsed_artifacts.as_ref()?;
    if artifacts.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    if let Some(summary) = artifacts
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    {
        lines.push(format!("summary: {summary}"));
    }
    if !artifacts.files.is_empty() {
        let files = artifacts
            .files
            .iter()
            .take(8)
            .map(|entry| match entry.notes.as_deref().map(str::trim) {
                Some(notes) if !notes.is_empty() => format!("{} ({notes})", entry.path),
                _ => entry.path.clone(),
            })
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!("files: {files}"));
    }
    if !artifacts.diffs.is_empty() {
        let diffs = artifacts
            .diffs
            .iter()
            .take(8)
            .map(|entry| match entry.path.as_deref().map(str::trim) {
                Some(path) if !path.is_empty() => format!("{path}: {}", entry.summary),
                _ => entry.summary.clone(),
            })
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!("diffs: {diffs}"));
    }
    if !artifacts.commands.is_empty() {
        let commands = artifacts
            .commands
            .iter()
            .take(8)
            .map(|entry| match entry.purpose.as_deref().map(str::trim) {
                Some(purpose) if !purpose.is_empty() => format!("{} ({purpose})", entry.cmd),
                _ => entry.cmd.clone(),
            })
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!("commands: {commands}"));
    }
    if !artifacts.risks.is_empty() {
        let risks = artifacts
            .risks
            .iter()
            .take(8)
            .map(|entry| {
                let prefix = entry
                    .level
                    .as_deref()
                    .map(str::trim)
                    .filter(|level| !level.is_empty())
                    .map(|level| format!("{level}: "))
                    .unwrap_or_default();
                let mitigation = entry
                    .mitigation
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(|text| format!(" (mitigation: {text})"))
                    .unwrap_or_default();
                format!("{prefix}{}{}", entry.item, mitigation)
            })
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!("risks: {risks}"));
    }
    if !artifacts.notes.is_empty() {
        lines.push(format!(
            "notes: {}",
            artifacts
                .notes
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    lines.push(format!(
        "artifact_path: .nit/swarm/{mission_id}/tasks/{}/artifacts.json",
        sanitize_for_filename(&task.id)
    ));
    Some(lines.join("\n"))
}

pub(super) fn parse_task_artifacts(task_id: &str, message: &str) -> Option<SwarmTaskArtifacts> {
    let mut merged = SwarmTaskArtifacts::default();
    let mut found = false;

    // Primary: look in fenced ```json blocks.
    for json in extract_json_code_blocks(message) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
            continue;
        };
        if let Some(parsed) = parse_task_artifacts_value(task_id, &value) {
            merge_task_artifacts(&mut merged, parsed);
            found = true;
        }
    }

    // Fallback: scan for raw JSON objects containing "swarm_artifacts" in the
    // message body.  Agents sometimes emit the JSON without a code fence, or
    // use a plain ``` fence instead of ```json.
    if !found {
        let text = message.trim();
        let mut search_from = 0;
        while let Some(start) = text[search_from..]
            .find(r#""type":"#)
            .or_else(|| text[search_from..].find(r#""type" :"#))
        {
            let abs_start = search_from + start;
            // Walk backward to find the opening brace.
            let obj_start = match text[..abs_start].rfind('{') {
                Some(s) => s,
                None => {
                    search_from = abs_start + 1;
                    continue;
                }
            };
            // Walk forward to find the matching closing brace.
            let mut depth = 0i32;
            let mut obj_end = None;
            for (i, ch) in text[obj_start..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            obj_end = Some(obj_start + i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end) = obj_end else {
                search_from = abs_start + 1;
                continue;
            };
            let candidate = &text[obj_start..=end];
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(candidate) {
                if let Some(parsed) = parse_task_artifacts_value(task_id, &value) {
                    merge_task_artifacts(&mut merged, parsed);
                    found = true;
                }
            }
            search_from = end + 1;
        }
    }

    if found && !merged.is_empty() {
        Some(merged)
    } else {
        None
    }
}

fn parse_task_artifacts_value(
    task_id: &str,
    value: &serde_json::Value,
) -> Option<SwarmTaskArtifacts> {
    let object = value.as_object()?;
    let typed = object
        .get("type")
        .and_then(|value| value.as_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("swarm_artifacts"));
    let has_artifacts = object.contains_key("artifacts");
    if !typed && !has_artifacts {
        return None;
    }
    if typed
        && object
            .get("version")
            .and_then(|value| value.as_u64())
            .is_some_and(|version| version != 1)
    {
        return None;
    }
    if let Some(owner) = object.get("task_id").and_then(|value| value.as_str()) {
        let owner = owner.trim();
        if !owner.is_empty() && owner != task_id {
            return None;
        }
    }

    let mut parsed = SwarmTaskArtifacts {
        summary: object
            .get("summary")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
            .map(ToString::to_string),
        ..SwarmTaskArtifacts::default()
    };

    let source = object.get("artifacts").unwrap_or(value);
    let source_obj = source.as_object()?;

    parsed.files = parse_artifact_files(source_obj.get("files"));
    parsed.diffs = parse_artifact_diffs(source_obj.get("diffs"));
    parsed.commands = parse_artifact_commands(source_obj.get("commands"));
    parsed.risks = parse_artifact_risks(source_obj.get("risks"));
    parsed.notes = parse_artifact_notes(source_obj.get("notes"));
    parsed.findings = parse_artifact_findings(source_obj.get("findings"));

    if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    }
}

fn parse_artifact_files(value: Option<&serde_json::Value>) -> Vec<SwarmArtifactFile> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(path) = item.as_str().map(str::trim).filter(|path| !path.is_empty()) {
            out.push(SwarmArtifactFile {
                path: path.to_string(),
                notes: None,
            });
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(path) = obj
            .get("path")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        let notes = obj
            .get("notes")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|notes| !notes.is_empty())
            .map(ToString::to_string);
        out.push(SwarmArtifactFile {
            path: path.to_string(),
            notes,
        });
    }
    out
}

fn parse_artifact_diffs(value: Option<&serde_json::Value>) -> Vec<SwarmArtifactDiff> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(summary) = item
            .as_str()
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
        {
            out.push(SwarmArtifactDiff {
                path: None,
                summary: summary.to_string(),
            });
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        let summary = obj
            .get("summary")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|summary| !summary.is_empty());
        let path = obj
            .get("path")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(ToString::to_string);
        let summary = summary.map(ToString::to_string).or_else(|| path.clone());
        let Some(summary) = summary else {
            continue;
        };
        out.push(SwarmArtifactDiff { path, summary });
    }
    out
}

fn parse_artifact_commands(value: Option<&serde_json::Value>) -> Vec<SwarmArtifactCommand> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(cmd) = item.as_str().map(str::trim).filter(|cmd| !cmd.is_empty()) {
            out.push(SwarmArtifactCommand {
                cmd: cmd.to_string(),
                purpose: None,
            });
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(cmd) = obj
            .get("cmd")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|cmd| !cmd.is_empty())
        else {
            continue;
        };
        let purpose = obj
            .get("purpose")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|purpose| !purpose.is_empty())
            .map(ToString::to_string);
        out.push(SwarmArtifactCommand {
            cmd: cmd.to_string(),
            purpose,
        });
    }
    out
}

fn parse_artifact_risks(value: Option<&serde_json::Value>) -> Vec<SwarmArtifactRisk> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(item_text) = item
            .as_str()
            .map(str::trim)
            .filter(|item_text| !item_text.is_empty())
        {
            out.push(SwarmArtifactRisk {
                level: None,
                item: item_text.to_string(),
                mitigation: None,
            });
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(item_text) = obj
            .get("item")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|item_text| !item_text.is_empty())
        else {
            continue;
        };
        let level = obj
            .get("level")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|level| !level.is_empty())
            .map(ToString::to_string);
        let mitigation = obj
            .get("mitigation")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|mitigation| !mitigation.is_empty())
            .map(ToString::to_string);
        out.push(SwarmArtifactRisk {
            level,
            item: item_text.to_string(),
            mitigation,
        });
    }
    out
}

fn parse_artifact_findings(value: Option<&serde_json::Value>) -> Vec<SwarmArtifactFinding> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        let Some(obj) = item.as_object() else {
            // Bare strings aren't useful as findings — without a file we
            // can't scope the retry. Skip silently.
            continue;
        };
        let Some(file) = obj
            .get("file")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|file| !file.is_empty())
        else {
            continue;
        };
        let Some(issue) = obj
            .get("issue")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|issue| !issue.is_empty())
        else {
            continue;
        };
        let line = obj
            .get("line")
            .and_then(|value| value.as_u64())
            .and_then(|n| u32::try_from(n).ok());
        let severity = obj
            .get("severity")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|severity| !severity.is_empty())
            .map(ToString::to_string);
        let category = obj
            .get("category")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|category| !category.is_empty())
            .map(|c| c.to_ascii_lowercase());
        let suggestion = obj
            .get("suggestion")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|suggestion| !suggestion.is_empty())
            .map(ToString::to_string);
        out.push(SwarmArtifactFinding {
            file: file.to_string(),
            line,
            severity,
            issue: issue.to_string(),
            category,
            suggestion,
        });
    }
    out
}

fn parse_artifact_notes(value: Option<&serde_json::Value>) -> Vec<String> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(note) = item.as_str().map(str::trim).filter(|note| !note.is_empty()) {
            out.push(note.to_string());
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        if let Some(note) = obj
            .get("note")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|note| !note.is_empty())
        {
            out.push(note.to_string());
        }
    }
    out
}

pub(super) fn merge_task_artifacts(dst: &mut SwarmTaskArtifacts, src: SwarmTaskArtifacts) {
    if let Some(summary) = src
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    {
        dst.summary = Some(summary.to_string());
    }

    dedup_extend(&mut dst.files, src.files, |entry| {
        entry.path.to_ascii_lowercase()
    });

    let diff_key = |entry: &SwarmArtifactDiff| {
        format!(
            "{}|{}",
            entry
                .path
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase(),
            entry.summary.to_ascii_lowercase()
        )
    };
    let mut seen_diffs = dst.diffs.iter().map(diff_key).collect::<HashSet<_>>();
    for entry in src.diffs {
        let key = diff_key(&entry);
        if key == "|" || !seen_diffs.insert(key) {
            continue;
        }
        dst.diffs.push(entry);
    }

    dedup_extend(&mut dst.commands, src.commands, |entry| {
        entry.cmd.to_ascii_lowercase()
    });
    dedup_extend(&mut dst.risks, src.risks, |entry| {
        entry.item.to_ascii_lowercase()
    });
    dedup_extend(&mut dst.notes, src.notes, |note| note.to_ascii_lowercase());
    // Findings dedup on file+line+issue so two verifier blocks pointing
    // at the same site don't generate two retry tasks for one fix.
    dedup_extend(&mut dst.findings, src.findings, |finding| {
        format!(
            "{}|{}|{}",
            finding.file.to_ascii_lowercase(),
            finding.line.map(|n| n.to_string()).unwrap_or_default(),
            finding.issue.to_ascii_lowercase()
        )
    });
}

fn dedup_extend<T>(dst: &mut Vec<T>, src: Vec<T>, key: impl Fn(&T) -> String) {
    let mut seen: HashSet<String> = dst.iter().map(&key).collect();
    for item in src {
        let k = key(&item);
        if k.is_empty() || !seen.insert(k) {
            continue;
        }
        dst.push(item);
    }
}
