/// Per-pane agent_id format: `<base>#mp-pane-NN` (zero-padded). The
/// `#mp-pane-` infix is unambiguous against existing `#swarm-...-clone-NN`,
/// `#chat-clone-NN`, and `#shadow-RUN-ROLE` conventions in
/// `crates/nit-tui/src/swarm/clones.rs`. Zero-padding matches
/// `#chat-clone-NN` so pane sort order survives N >= 10.
pub const PANE_SEPARATOR: &str = "#mp-pane-";

pub fn pane_agent_id(base: &str, idx: usize) -> String {
    format!("{base}{PANE_SEPARATOR}{idx:02}")
}

pub fn parse_pane_agent_id(id: &str) -> Option<(&str, usize)> {
    let (base, rest) = id.split_once(PANE_SEPARATOR)?;
    rest.parse().ok().map(|n| (base, n))
}

pub fn is_multipane_pane_id(id: &str) -> bool {
    parse_pane_agent_id(id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swarm::{chat_clone_base_id, is_any_clone_agent_id};

    #[test]
    fn pane_agent_id_zero_pads() {
        assert_eq!(
            pane_agent_id("claude-haiku-4-5", 0),
            "claude-haiku-4-5#mp-pane-00"
        );
        assert_eq!(
            pane_agent_id("claude-haiku-4-5", 3),
            "claude-haiku-4-5#mp-pane-03"
        );
        assert_eq!(pane_agent_id("gpt-5", 12), "gpt-5#mp-pane-12");
    }

    #[test]
    fn parse_pane_agent_id_roundtrips() {
        for (base, idx) in [
            ("claude-haiku-4-5", 0usize),
            ("gpt-5", 7),
            ("gemini-2.5-pro", 31),
        ] {
            let id = pane_agent_id(base, idx);
            let (parsed_base, parsed_idx) = parse_pane_agent_id(&id).expect("parses");
            assert_eq!(parsed_base, base);
            assert_eq!(parsed_idx, idx);
        }
    }

    #[test]
    fn is_multipane_pane_id_works() {
        assert!(is_multipane_pane_id("claude-haiku-4-5#mp-pane-00"));
        assert!(!is_multipane_pane_id("claude-haiku-4-5"));
        assert!(!is_multipane_pane_id("claude-haiku-4-5#chat-clone-01"));
        assert!(!is_multipane_pane_id(
            "claude-haiku-4-5#swarm-mission-clone-01"
        ));
    }

    #[test]
    fn clone_predicates_reject_pane_id() {
        // Pane lanes are long-lived per-launch sessions, not transient
        // clones. They MUST NOT be classified by the clone heuristics in
        // swarm::clones — that would mis-route apply_swarm_task_role,
        // cleanup_idle_chat_clone, and the integrator-budget heuristic.
        let pane = pane_agent_id("claude-haiku-4-5", 3);
        assert_eq!(chat_clone_base_id(&pane), None);
        assert!(!is_any_clone_agent_id(&pane));
    }
}
