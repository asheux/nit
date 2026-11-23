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
    (
        "swarm-propose",
        "Landscape analysis, structural plans, and file handoff",
        include_str!("skills/swarm-propose.md"),
    ),
    (
        "swarm-judge",
        "Proposal comparison and verdict scope",
        include_str!("skills/swarm-judge.md"),
    ),
    (
        "swarm-integrate",
        "Implementation scope, blockers, and structural checks",
        include_str!("skills/swarm-integrate.md"),
    ),
];

pub fn catalog() -> &'static str {
    static INDEX: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        SKILLS
            .iter()
            .map(|(name, summary, _)| format!("{name}  {summary}"))
            .collect::<Vec<_>>()
            .join("\n")
    });
    &INDEX
}

pub fn read(name: &str) -> Option<&'static str> {
    SKILLS
        .iter()
        .find(|entry| entry.0 == name)
        .map(|entry| entry.2)
}

pub fn lookup(name: Option<&str>) -> Result<String, String> {
    match name {
        None => Ok(catalog().into()),
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
    nit_utils::write_atomic(&dir.join("INDEX.md"), |file| {
        file.write_all(catalog().as_bytes())
    })
}

/// Share one directory across backends, including sessions without a shell or PATH entry.
pub fn reference_dir() -> Option<&'static std::path::Path> {
    static DIR: std::sync::LazyLock<Option<std::path::PathBuf>> = std::sync::LazyLock::new(|| {
        let path = nit_utils::paths::cache_dir()?.join("nitskills");
        write_references(&path).ok()?;
        Some(path)
    });
    DIR.as_deref()
}

pub fn reference_hint() -> String {
    if let Some(dir) = reference_dir() {
        return format!(
            "NitSkills: read INDEX.md or NAME.md in {} on demand.",
            dir.display()
        );
    }
    match std::env::current_exe() {
        Ok(path) => format!(
            "NitSkills: run the executable at {} with arguments skills [NAME].",
            path.display()
        ),
        Err(_) => "NitSkills: use nit_skill or run nit skills [NAME].".into(),
    }
}
