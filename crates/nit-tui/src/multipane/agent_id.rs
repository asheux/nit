/// Per-pane agent_id format: `<base>#mp-pane-NN` (zero-padded). Disjoint
/// from `#swarm-…-clone-NN`, `#chat-clone-NN`, and `#shadow-RUN-ROLE`
/// conventions in `crates/nit-tui/src/swarm/clones.rs`.
pub const PANE_SEPARATOR: &str = "#mp-pane-";

pub fn pane_agent_id(base: &str, idx: usize) -> String {
    format!("{base}{PANE_SEPARATOR}{idx:02}")
}

/// Stable per-pane synthetic chat mission id. Pure function of `pane_id`,
/// recomputed on load — never persisted directly.
pub fn pane_chat_mission_id(idx: usize) -> String {
    format!("mp-pane-{idx:02}-chat")
}

pub fn is_pane_chat_mission_id(id: &str) -> bool {
    id.starts_with("mp-pane-") && id.ends_with("-chat")
}

/// The trailing `#suffix` tolerance is load-bearing for swarm clones
/// (`claude#mp-pane-00#swarm-mis-…-clone-01`) — bare `parse::<usize>()`
/// would reject them and the abort / broadcast scope predicates would
/// silently drop every swarm clone.
pub fn parse_pane_agent_id(id: &str) -> Option<(&str, usize)> {
    let (base, rest) = id.split_once(PANE_SEPARATOR)?;
    let digits_end = rest.find('#').unwrap_or(rest.len());
    rest[..digits_end].parse().ok().map(|n| (base, n))
}

pub fn is_multipane_pane_id(id: &str) -> bool {
    parse_pane_agent_id(id).is_some()
}

pub fn pane_owns_agent(agent_id: &str, pane_idx: usize) -> bool {
    parse_pane_agent_id(agent_id).is_some_and(|(_, idx)| idx == pane_idx)
}

#[cfg(test)]
#[path = "../tests/multipane_agent_id.rs"]
mod tests;
