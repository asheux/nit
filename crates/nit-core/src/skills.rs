//! Built-in reference material shared by the CLI, chat, and MCP.

#[derive(Debug)]
pub struct Skill {
    pub name: &'static str,
    pub title: &'static str,
    pub group: &'static str,
    pub summary: &'static str,
    pub body: &'static str,
}

macro_rules! skill {
    ($group:literal, $name:literal, $title:literal, $summary:literal) => {
        Skill {
            name: $name,
            title: $title,
            group: $group,
            summary: $summary,
            body: include_str!(concat!("skills/", $name, ".md")),
        }
    };
}

pub fn definitions() -> &'static [Skill] {
    &[
        skill!(
            "Code structure",
            "genome-quality",
            "Interpret a genome tier",
            "Read a file’s simulation score and understand its limits."
        ),
        skill!(
            "Code structure",
            "genome-encoders",
            "Compare encoders",
            "Trace a score change back to the source features each encoder uses."
        ),
        skill!(
            "Swarm workflow",
            "swarm-plan",
            "Plan a mission",
            "Assign owners, dependencies and acceptance checks."
        ),
        skill!(
            "Swarm workflow",
            "swarm-propose",
            "Propose a change",
            "Compare implementation options and identify the files each would touch."
        ),
        skill!(
            "Swarm workflow",
            "swarm-judge",
            "Judge proposals",
            "Choose an approach and record the reasons for the writer."
        ),
        skill!(
            "Swarm workflow",
            "swarm-integrate",
            "Implement the verdict",
            "Carry the accepted change through callers, tests and integration points."
        ),
        skill!(
            "Swarm workflow",
            "swarm-review",
            "Review a change",
            "Trace user-visible regressions and report actionable findings."
        ),
        skill!(
            "Swarm workflow",
            "swarm-test",
            "Test a change",
            "Choose focused checks and distinguish results from coverage gaps."
        ),
        skill!(
            "Swarm workflow",
            "swarm-verification",
            "Verify a mission",
            "Interpret gate results and retry the checks that failed."
        ),
        skill!(
            "Swarm workflow",
            "swarm-handoff",
            "Hand off evidence",
            "Package changed paths, commands and findings for the next agent."
        ),
        skill!(
            "Research",
            "research-evidence",
            "Compare research evidence",
            "Weigh sources, state assumptions and rank candidate strategies."
        ),
        skill!(
            "Research",
            "research-experiment",
            "Run an experiment",
            "Define a baseline, record measurements and bound the conclusion."
        ),
        skill!(
            "Workspace",
            "workspace-scope",
            "Define file scope",
            "Separate files that need edits from paths inspected for context."
        ),
        skill!(
            "Workspace",
            "workspace-claims",
            "Coordinate file ownership",
            "Check claims and revalidate assumptions when edits overlap."
        ),
        skill!(
            "Workspace",
            "shadow-review",
            "Compare two approaches",
            "Use independent proposals and a review to guide one agent’s turn."
        ),
        skill!(
            "Workspace",
            "multiway-candidates",
            "Compare worktrees",
            "Evaluate candidate commits against the same checks in isolated worktrees."
        ),
    ]
}

/// Catalog entries in display order, without allocating the reference bodies.
pub fn entries() -> impl ExactSizeIterator<Item = (&'static str, &'static str)> {
    definitions()
        .iter()
        .map(|skill| (skill.name, skill.summary))
}

pub fn catalog() -> &'static str {
    static INDEX: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        entries()
            .map(|(name, summary)| format!("{name}  {summary}"))
            .collect::<Vec<_>>()
            .join("\n")
    });
    &INDEX
}

pub fn read(name: &str) -> Option<&'static str> {
    definitions()
        .iter()
        .find(|skill| skill.name == name)
        .map(|skill| skill.body)
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
    let references = definitions()
        .iter()
        .map(|skill| (skill.name, skill.body))
        .chain(std::iter::once(("INDEX", catalog())));
    for (name, body) in references {
        let path = dir.join(format!("{name}.md"));
        if std::fs::symlink_metadata(&path)
            .is_ok_and(|meta| meta.is_file() && meta.len() == body.len() as u64)
            && std::fs::read(&path).is_ok_and(|existing| existing == body.as_bytes())
        {
            continue;
        }
        nit_utils::write_atomic(&path, |file| file.write_all(body.as_bytes()))?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::SystemTime};

    #[test]
    fn reference_install_reuses_matching_files_and_repairs_stale_content() {
        let dir = crate::test_helpers::temp_dir("skill-cache");
        write_references(&dir).unwrap();
        let index = dir.join("INDEX.md");
        fs::File::options()
            .write(true)
            .open(&index)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH)
            .unwrap();
        let before = fs::metadata(&index).unwrap().modified().unwrap();
        let body = read("swarm-judge").unwrap();
        let judge = dir.join("swarm-judge.md");
        fs::write(&judge, vec![b' '; body.len()]).unwrap();

        write_references(&dir).unwrap();
        assert_eq!(fs::metadata(index).unwrap().modified().unwrap(), before);
        assert_eq!(fs::read_to_string(judge).unwrap(), body);
        fs::remove_dir_all(dir).unwrap();
    }
}
