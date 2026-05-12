use std::collections::HashMap;

use super::constants::{
    NIT_PROMPT_TIERS_ENV, PROMPT_BUDGET_DEFAULT, PROMPT_BUDGET_INTEGRATE, PROMPT_BUDGET_JUDGE,
    PROMPT_BUDGET_PROPOSE, PROMPT_BUDGET_RESEARCH, PROMPT_BUDGET_REVIEW, PROMPT_BUDGET_TEST,
};
use super::{normalize_role_label, per_dep_budget, SwarmTask};

#[derive(Clone, Debug)]
pub(crate) struct PromptBudgets {
    pub(crate) integrate: usize,
    pub(crate) judge: usize,
    pub(crate) propose: usize,
    pub(crate) review: usize,
    pub(crate) test: usize,
    pub(crate) research: usize,
    pub(crate) default: usize,
    pub(crate) tiers_enabled: bool,
}

impl Default for PromptBudgets {
    fn default() -> Self {
        Self {
            integrate: PROMPT_BUDGET_INTEGRATE,
            judge: PROMPT_BUDGET_JUDGE,
            propose: PROMPT_BUDGET_PROPOSE,
            review: PROMPT_BUDGET_REVIEW,
            test: PROMPT_BUDGET_TEST,
            research: PROMPT_BUDGET_RESEARCH,
            default: PROMPT_BUDGET_DEFAULT,
            tiers_enabled: true,
        }
    }
}

impl PromptBudgets {
    pub(crate) fn from_env() -> Self {
        let tiers_enabled = read_tiers_enabled();
        Self {
            integrate: read_role_override("INTEGRATE", PROMPT_BUDGET_INTEGRATE),
            judge: read_role_override("JUDGE", PROMPT_BUDGET_JUDGE),
            propose: read_role_override("PROPOSE", PROMPT_BUDGET_PROPOSE),
            review: read_role_override("REVIEW", PROMPT_BUDGET_REVIEW),
            test: read_role_override("TEST", PROMPT_BUDGET_TEST),
            research: read_role_override("RESEARCH", PROMPT_BUDGET_RESEARCH),
            default: read_role_override("DEFAULT", PROMPT_BUDGET_DEFAULT),
            tiers_enabled,
        }
    }

    pub(crate) fn for_role(&self, role: Option<&str>, writes: bool) -> usize {
        if !self.tiers_enabled {
            return usize::MAX;
        }
        match role.and_then(normalize_role_label).as_deref() {
            Some("integrate") => self.integrate,
            Some("judge") => self.judge,
            Some("propose") => self.propose,
            Some("review") => self.review,
            Some("test") => self.test,
            Some("research" | "computational-research") => self.research,
            _ if writes => self.integrate,
            _ => self.default,
        }
    }

    pub(crate) fn effective_budget(
        &self,
        role: Option<&str>,
        writes: bool,
        overrides: &HashMap<String, usize>,
    ) -> usize {
        if !self.tiers_enabled {
            return usize::MAX;
        }
        if let Some(canonical) = role.and_then(normalize_role_label) {
            if let Some(&v) = overrides.get(canonical.as_str()) {
                return v;
            }
        }
        self.for_role(role, writes)
    }
}

fn read_tiers_enabled() -> bool {
    match std::env::var(NIT_PROMPT_TIERS_ENV) {
        Ok(raw) => !matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

fn read_role_override(role_token: &str, default: usize) -> usize {
    let key = format!("NIT_PROMPT_BUDGET_{role_token}");
    match std::env::var(&key) {
        Ok(raw) => raw.trim().parse::<usize>().unwrap_or(default),
        Err(_) => default,
    }
}

pub(crate) fn parse_override_token(token: &str) -> Result<(String, usize), String> {
    let trimmed = token.trim();
    let (role, value) = trimmed
        .split_once(':')
        .ok_or_else(|| format!("budget token `{trimmed}` must be ROLE:N"))?;
    let role = role.trim();
    if role.is_empty() {
        return Err(format!("budget token `{trimmed}` is missing a role"));
    }
    let canonical = normalize_role_label(role)
        .ok_or_else(|| format!("budget token `{trimmed}` has unknown role `{role}`"))?;
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("budget token `{trimmed}` is missing a value"));
    }
    let (digits, multiplier) = match value
        .chars()
        .last()
        .map(|ch| ch.to_ascii_lowercase())
        .filter(|ch| *ch == 'k')
    {
        Some(_) => (&value[..value.len() - 1], 1024usize),
        None => (value, 1usize),
    };
    if digits.is_empty() {
        return Err(format!(
            "budget token `{trimmed}` is missing digits before `k`"
        ));
    }
    let base: usize = digits
        .parse()
        .map_err(|_| format!("budget token `{trimmed}` has non-numeric value `{value}`"))?;
    let resolved = base
        .checked_mul(multiplier)
        .ok_or_else(|| format!("budget token `{trimmed}` overflowed"))?;
    Ok((canonical, resolved))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stage3Diag {
    pub(crate) orig_kb: usize,
    pub(crate) new_kb: usize,
    pub(crate) is_integrate: bool,
}

pub(crate) fn apply_prompt_budget(
    prompt: &mut String,
    budget: usize,
    task: &SwarmTask,
    effective_dep_count: usize,
) -> Option<Stage3Diag> {
    if budget == usize::MAX || prompt.len() <= budget {
        return None;
    }
    halve_dep_payloads(prompt, task, effective_dep_count);
    if prompt.len() <= budget {
        return None;
    }
    drop_dep_payloads_matching(prompt, budget, "propose");
    if prompt.len() <= budget {
        return None;
    }
    drop_dep_payloads_matching(prompt, budget, "judge");
    if prompt.len() <= budget {
        return None;
    }
    shrink_genome_landscape(prompt, task)
}

pub(crate) fn stage3_diag_message(diag: &Stage3Diag) -> Option<String> {
    if !diag.is_integrate {
        return None;
    }
    Some(format!(
        "prompt budget tier integrate: landscape compressed (orig {} KB → {} KB)",
        diag.orig_kb, diag.new_kb
    ))
}

fn halve_dep_payloads(prompt: &mut String, task: &SwarmTask, effective_dep_count: usize) {
    let per_dep = per_dep_budget(task.role.as_deref(), task.writes, effective_dep_count);
    let halved = per_dep / 2;
    if halved == 0 {
        return;
    }
    let Some(section) = find_dep_section_bounds(prompt) else {
        return;
    };
    let blocks = collect_dep_blocks(prompt, &section);
    if blocks.is_empty() {
        return;
    }
    let mut rebuilt = String::with_capacity(section.body_end - section.body_start);
    rebuilt.push_str(&prompt[section.body_start..blocks[0].header_start]);
    for block in &blocks {
        rebuilt.push_str(&prompt[block.header_start..block.payload_start]);
        let payload = &prompt[block.payload_start..block.end];
        rebuilt.push_str(&truncate_with_marker(payload, halved));
        if !rebuilt.ends_with('\n') {
            rebuilt.push('\n');
        }
    }
    let mut new_prompt = String::with_capacity(prompt.len());
    new_prompt.push_str(&prompt[..section.body_start]);
    new_prompt.push_str(&rebuilt);
    new_prompt.push_str(&prompt[section.body_end..]);
    *prompt = new_prompt;
}

fn drop_dep_payloads_matching(prompt: &mut String, budget: usize, role_filter: &str) {
    loop {
        if prompt.len() <= budget {
            return;
        }
        let Some(section) = find_dep_section_bounds(prompt) else {
            return;
        };
        let blocks = collect_dep_blocks(prompt, &section);
        let Some(target) = blocks
            .iter()
            .find(|b| dep_label_role_matches(&b.label, role_filter))
        else {
            return;
        };
        let breadcrumb = format!(
            "\n---\nDEP: {}\n[Dropped {role_filter} dep payload — see .nit/swarm/<mission>/tasks/<dep>/artifacts.json]\n",
            target.label
        );
        let mut new_prompt = String::with_capacity(prompt.len());
        new_prompt.push_str(&prompt[..target.header_start]);
        new_prompt.push_str(&breadcrumb);
        new_prompt.push_str(&prompt[target.end..]);
        if new_prompt.len() >= prompt.len() {
            return;
        }
        *prompt = new_prompt;
    }
}

const LANDSCAPE_HEADER_MARKER: &str = "## GENOME LANDSCAPE";

fn shrink_genome_landscape(prompt: &mut String, task: &SwarmTask) -> Option<Stage3Diag> {
    let header_start = prompt.find(LANDSCAPE_HEADER_MARKER)?;
    let header_line_end = prompt[header_start..]
        .find('\n')
        .map(|i| header_start + i + 1)
        .unwrap_or(prompt.len());
    let block_end = find_next_top_level_section(prompt, header_line_end).unwrap_or(prompt.len());
    let orig_block_len = block_end - header_start;
    if orig_block_len == 0 {
        return None;
    }
    let summary = summarize_genome_landscape(&prompt[header_start..block_end]);
    if summary.len() >= orig_block_len {
        return None;
    }
    let is_integrate = task
        .role
        .as_deref()
        .and_then(normalize_role_label)
        .as_deref()
        == Some("integrate");
    let diag = Stage3Diag {
        orig_kb: orig_block_len / 1024,
        new_kb: summary.len() / 1024,
        is_integrate,
    };
    let mut new_prompt = String::with_capacity(prompt.len());
    new_prompt.push_str(&prompt[..header_start]);
    new_prompt.push_str(&summary);
    new_prompt.push_str(&prompt[block_end..]);
    *prompt = new_prompt;
    Some(diag)
}

fn find_next_top_level_section(prompt: &str, after: usize) -> Option<usize> {
    prompt[after..].find("\n## ").map(|rel| after + rel + 1)
}

fn summarize_genome_landscape(block: &str) -> String {
    let header_end = block.find('\n').map(|i| i + 1).unwrap_or(block.len());
    let mut out = String::with_capacity(256);
    out.push_str(&block[..header_end]);
    out.push_str(
        "(landscape compressed under prompt budget — top-10 worst-tier files listed below; full landscape on disk under .nit/swarm/<mission>/landscape.txt)\n\n",
    );
    let rows = collect_landscape_rows(block);
    if rows.is_empty() {
        return out;
    }
    for (path, tier, consistency) in rows.iter().take(10) {
        out.push_str(&format!(
            "- {path}: tier {tier}, consistency {consistency}\n"
        ));
    }
    out
}

fn collect_landscape_rows(block: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut lines = block.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if !(trimmed.starts_with("---") && trimmed.ends_with("---")) {
            continue;
        }
        let path = trimmed.trim_matches('-').trim().to_string();
        if path.is_empty() {
            continue;
        }
        let mut tier = String::from("?");
        let mut consistency = String::from("?");
        for _ in 0..4 {
            let Some(next) = lines.peek() else { break };
            let next_trimmed = next.trim();
            if let Some(rest) = next_trimmed.strip_prefix("Tier:") {
                let parts: Vec<&str> = rest.trim().splitn(2, ',').collect();
                if let Some(first) = parts.first() {
                    tier = first.trim().to_string();
                }
                for piece in rest.split(',').map(str::trim) {
                    if let Some(rest) = piece.strip_prefix("consistency:") {
                        consistency = rest.trim().to_string();
                    }
                }
                let _ = lines.next();
                break;
            }
            let _ = lines.next();
        }
        out.push((path, tier, consistency));
    }
    out
}

struct DepSection {
    body_start: usize,
    body_end: usize,
}

struct DepBlock {
    header_start: usize,
    payload_start: usize,
    end: usize,
    label: String,
}

const DEP_SECTION_MARKERS: &[&str] = &[
    "\n## IMPLEMENTATION PLAN (BINDING — follow verbatim)",
    "\nDependency outputs (",
    "\nDependency outputs:",
];

const SECTION_TERMINATORS: &[&str] = &[
    "\nRespond with:",
    "\n## STRUCTURED ARTIFACTS",
    "\n## SIGN-OFF",
    "\n## GENOME LANDSCAPE",
];

fn find_dep_section_bounds(prompt: &str) -> Option<DepSection> {
    let header_start = DEP_SECTION_MARKERS
        .iter()
        .filter_map(|m| prompt.find(m))
        .min()?;
    let body_start = prompt[header_start..]
        .find('\n')
        .map(|i| header_start + i + 1)
        .unwrap_or(prompt.len());
    let mut body_end = prompt.len();
    for terminator in SECTION_TERMINATORS {
        if let Some(rel) = prompt[body_start..].find(terminator) {
            body_end = body_end.min(body_start + rel);
        }
    }
    Some(DepSection {
        body_start,
        body_end,
    })
}

fn collect_dep_blocks(prompt: &str, section: &DepSection) -> Vec<DepBlock> {
    const SEPARATOR: &str = "\n---\nDEP: ";
    let body = &prompt[section.body_start..section.body_end];
    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel) = body[cursor..].find(SEPARATOR) {
        let abs_match = section.body_start + cursor + rel;
        let header_start = abs_match + 1;
        let label_start = abs_match + SEPARATOR.len();
        let label_end = prompt[label_start..]
            .find('\n')
            .map(|i| label_start + i)
            .unwrap_or(section.body_end);
        let payload_start = (label_end + 1).min(section.body_end);
        let next_rel = body[(cursor + rel + 1)..].find(SEPARATOR);
        let end = match next_rel {
            Some(rel2) => section.body_start + cursor + rel + 1 + rel2,
            None => section.body_end,
        };
        let label = prompt[label_start..label_end].trim().to_string();
        blocks.push(DepBlock {
            header_start,
            payload_start,
            end,
            label,
        });
        let advance = end - section.body_start;
        cursor = if advance > cursor {
            advance
        } else {
            cursor + rel + 1
        };
        if cursor >= body.len() {
            break;
        }
    }
    blocks
}

fn dep_label_role_matches(label: &str, role_filter: &str) -> bool {
    label
        .split_whitespace()
        .next()
        .map(|first| first.to_ascii_lowercase().starts_with(role_filter))
        .unwrap_or(false)
}

fn truncate_with_marker(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let mut out = String::with_capacity(boundary + 32);
    out.push_str(&text[..boundary]);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("[…truncated under prompt budget…]\n");
    out
}
