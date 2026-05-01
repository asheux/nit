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

/// Parse the pane index from an agent id of the form `<base>#mp-pane-NN`
/// or `<base>#mp-pane-NN<#suffix>`. The trailing `#suffix` tolerance is
/// load-bearing for swarm clones (`claude#mp-pane-00#swarm-mis-…-clone-01`)
/// — bare `parse::<usize>()` would otherwise reject them and the abort /
/// broadcast scope predicates below would silently drop every swarm clone.
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
mod tests {
    use super::*;
    use crate::swarm::{chat_clone_base_id, is_any_clone_agent_id};

    #[test]
    fn pane_agent_id_zero_pads_and_roundtrips() {
        for (base, idx) in [
            ("claude-haiku-4-5", 0usize),
            ("gpt-5", 7),
            ("gemini-2.5-pro", 31),
        ] {
            let id = pane_agent_id(base, idx);
            let want = format!("{base}#mp-pane-{idx:02}");
            assert_eq!(id, want);
            let (parsed_base, parsed_idx) = parse_pane_agent_id(&id).expect("parses");
            assert_eq!(parsed_base, base);
            assert_eq!(parsed_idx, idx);
        }
        assert_eq!(pane_agent_id("gpt-5", 12), "gpt-5#mp-pane-12");
    }

    #[test]
    fn pane_predicates_classify_lanes_clones_and_unrelated_ids() {
        // Pane lanes — accepted.
        let pane = pane_agent_id("claude-haiku-4-5", 3);
        assert!(is_multipane_pane_id(&pane));
        assert!(pane_owns_agent(&pane, 3));
        assert!(!pane_owns_agent(&pane, 0));
        // Swarm clones nest a `#swarm-…` suffix after the pane index —
        // load-bearing for /abort and @all routing.
        let clone = "claude-haiku-4-5#mp-pane-02#swarm-mis-001-clone-03";
        assert_eq!(parse_pane_agent_id(clone), Some(("claude-haiku-4-5", 2)));
        assert!(pane_owns_agent(clone, 2));
        assert!(!pane_owns_agent(clone, 0));
        // Non-pane ids — rejected by every predicate.
        for id in [
            "claude-haiku-4-5",
            "claude-haiku-4-5#chat-clone-01",
            "claude-haiku-4-5#swarm-mission-clone-01",
        ] {
            assert!(!is_multipane_pane_id(id), "{id}");
            assert!(!pane_owns_agent(id, 0), "{id}");
        }
        // The clone heuristics must NOT classify pane lanes — would
        // otherwise mis-route apply_swarm_task_role / cleanup_idle_chat_clone.
        assert_eq!(chat_clone_base_id(&pane), None);
        assert!(!is_any_clone_agent_id(&pane));
    }

    #[test]
    fn synthetic_chat_mission_id_format_and_recognition() {
        for idx in [0, 7, 31usize] {
            let mid = pane_chat_mission_id(idx);
            assert_eq!(mid, format!("mp-pane-{idx:02}-chat"));
            assert!(is_pane_chat_mission_id(&mid));
        }
        for non in [
            "mp-pane-00",
            "swarm-mis-001",
            "mp-pane-NN-chat-extra",
            "chat",
        ] {
            assert!(!is_pane_chat_mission_id(non), "{non}");
        }
    }
}
