//! Built-in reference material shared by the CLI, chat, and MCP.

const SKILLS: &[(&str, &str, &str)] = &[
    (
        "genome-quality",
        "Tier ladder and parsimony rules",
        include_str!("skills/genome-quality.md"),
    ),
    (
        "genome-encoders",
        "Encoder guide, regression attribution, and targets",
        include_str!("skills/genome-encoders.md"),
    ),
    (
        "swarm-verification",
        "Targeted verification, code hygiene, and gate retries",
        include_str!("skills/swarm-verification.md"),
    ),
];

pub fn catalog() -> String {
    SKILLS
        .iter()
        .map(|(name, summary, _)| format!("{name}  {summary}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn read(name: &str) -> Option<&'static str> {
    SKILLS
        .iter()
        .find(|entry| entry.0 == name)
        .map(|entry| entry.2)
}

pub fn lookup(name: Option<&str>) -> Result<String, String> {
    match name {
        None => Ok(catalog()),
        Some(name) => read(name).map(str::to_owned).ok_or_else(|| {
            format!(
                "Unknown NitSkill {name:?}. Available skills:\n{}",
                catalog()
            )
        }),
    }
}

/// Recognize only the standalone slash command and its optional skill name.
pub fn chat_reply(input: &str) -> Option<String> {
    let input = input.trim();
    let command = input.split_whitespace().next()?;
    if command != "/skills" {
        return None;
    }
    let name = input[command.len()..].trim();
    Some(lookup((!name.is_empty()).then_some(name)).unwrap_or_else(|error| error))
}

/// Make embedded references available to tools that can only read files.
pub fn write_references(dir: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    for (name, _, body) in SKILLS {
        let path = dir.join(format!("{name}.md"));
        nit_utils::write_atomic(&path, |file| file.write_all(body.as_bytes()))?;
    }
    Ok(())
}
