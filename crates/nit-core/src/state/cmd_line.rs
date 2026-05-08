use super::*;
use crate::{lab::AppKind, prompt::Prompt};

mod games_inspect;
mod helpers;

pub(super) use helpers::{apply_protocol_selection, apply_rule_selection};
use helpers::{is_help_command_tokens, lab_from_tokens, log_rule_list, log_rule_overview};

pub(super) fn handle_command_line(state: &mut AppState, input: &str) -> bool {
    let trimmed = input.trim();
    let cmd = trimmed.trim_start_matches(':').trim().to_lowercase();
    if cmd.is_empty() {
        return false;
    }
    let normalized = cmd
        .split_whitespace()
        .map(|token| token.trim_matches(':'))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let tokens: Vec<&str> = normalized;
    if let Some(target_lab) = lab_from_tokens(&tokens) {
        if target_lab != state.app_kind {
            state.status = Some(format!(
                "{} lab not active (current: {}). Use --lab {} to start.",
                target_lab.label(),
                state.app_kind.label(),
                target_lab
            ));
            return false;
        }
    }
    if is_help_command_tokens(&tokens) {
        state.show_help = true;
        state.help_scroll = 0;
        state.status = Some("Help opened".into());
        return false;
    }
    match tokens.as_slice() {
        ["substrate"] | ["sub"] | ["sig"] | ["signals"] => {
            state.show_substrate_overlay = true;
            state.substrate_overlay_tab = SubstrateOverlayTab::Signals;
            state.substrate_overlay_scroll = 0;
            false
        }
        ["claims"] => {
            state.show_substrate_overlay = true;
            state.substrate_overlay_tab = SubstrateOverlayTab::Claims;
            state.substrate_overlay_scroll = 0;
            false
        }
        ["assumptions"] | ["asm"] => {
            state.show_substrate_overlay = true;
            state.substrate_overlay_tab = SubstrateOverlayTab::Assumptions;
            state.substrate_overlay_scroll = 0;
            false
        }
        ["q"] | ["quit"] | ["exit"] => {
            if state.has_unsaved_editor_buffers() {
                state.prompt = Some(Prompt::ConfirmQuit);
                false
            } else {
                true
            }
        }
        ["tree"] | ["nittree"] | ["explore"] => {
            state.file_tree.open = !state.file_tree.open;
            if state.file_tree.open {
                state.file_tree.root = state.workspace_root.clone();
                state.focus = PaneId::Editor;
                state.mode = Mode::Normal;
                state.status = Some("NITTree opened".into());
            } else {
                state.status = Some("NITTree closed".into());
            }
            false
        }
        ["find"] | ["ff"] => {
            state
                .fuzzy_search
                .open(SearchMode::Files, state.workspace_root.clone());
            state.status = Some("Search: files".into());
            false
        }
        ["grep"] | ["rg"] | ["search"] => {
            state
                .fuzzy_search
                .open(SearchMode::Content, state.workspace_root.clone());
            state.status = Some("Search: content".into());
            false
        }
        ["close"] => {
            if state.fuzzy_search.open {
                state.fuzzy_search.close();
                state.status = Some("Search closed".into());
            }
            false
        }
        ["run"] => match state.app_kind {
            AppKind::Gol => {
                state.visualizer.pending_run = true;
                state.visualizer.pending_snapshot = true;
                state.status = Some("Petri dish queued".into());
                false
            }
            AppKind::Games => {
                state.games.pending_run_override = None;
                state.games.pending_family_run = None;
                state.games.family_building = false;
                state.games.pending_run = true;
                state.status = Some("Games tournament queued".into());
                false
            }
        },
        ["gol", "run"] | ["run", "gol"] | ["life", "run"] | ["gol", "start"] | ["run", "life"] => {
            state.visualizer.pending_run = true;
            state.visualizer.pending_snapshot = true;
            state.status = Some("Petri dish queued".into());
            false
        }
        _ if tokens.first() == Some(&"games")
            && tokens.get(1) == Some(&"run")
            && tokens.len() > 2 =>
        {
            let (force, family) = if tokens.get(2) == Some(&"force") {
                match tokens.get(3).copied() {
                    Some(family) => (true, family),
                    None => {
                        state.status = Some(
                            "Usage: :games run force <fsm|ca|tm> {params} (e.g. :games run force fsm {3, 2})"
                                .into(),
                        );
                        return false;
                    }
                }
            } else {
                (false, tokens[2])
            };

            if state.games.family_building {
                state.status = Some("Family run preparation already in progress".into());
                return false;
            }

            match build_family_run_override(state, family, trimmed, force) {
                Ok(request) => {
                    state.games.pending_run_override = None;
                    state.games.pending_run = false;
                    state.games.pending_family_run = Some(request);
                    state.games.family_building = true;
                    let mode = if force { "forced, " } else { "" };
                    state.status = Some(format!("Preparing family run ({mode}{family})..."));
                }
                Err(err) => {
                    state.games.pending_family_run = None;
                    state.games.family_building = false;
                    state.status = Some(err)
                }
            }
            false
        }
        ["games", "run"] | ["run", "games"] => {
            state.games.pending_run_override = None;
            state.games.pending_family_run = None;
            state.games.family_building = false;
            state.games.pending_run = true;
            state.status = Some("Games tournament queued".into());
            false
        }
        ["gol", "hide"] | ["hide", "gol"] => {
            state.visualizer.pending_hide = true;
            state.status = Some("Petri dish hiding".into());
            false
        }
        ["gol", "show"] | ["show", "gol"] => {
            state.visualizer.pending_show = true;
            state.status = Some("Petri dish showing".into());
            false
        }
        ["gol", "stop"] | ["life", "stop"] => {
            state.visualizer.pending_close = true;
            state.status = Some("Petri dish closing".into());
            false
        }
        ["run", "stop"] if state.app_kind == AppKind::Gol => {
            state.visualizer.pending_close = true;
            state.status = Some("Petri dish closing".into());
            false
        }
        ["games", "hide"] | ["hide", "games"] => {
            state.games.pending_hide = true;
            state.status = Some("Games tournament hiding".into());
            false
        }
        ["games", "show"] | ["show", "games"] => {
            state.games.pending_show = true;
            state.status = Some("Games tournament showing".into());
            false
        }
        ["games", "stop"] | ["stop", "games"] => {
            state.games.pending_close = true;
            state.status = Some("Games tournament closing".into());
            false
        }
        ["games", "status"] => {
            state.status = Some(format!("Games status: {:?}", state.games.status));
            false
        }
        ["games", "runs"] | ["games", "browse"] | ["games", "browser"] => {
            state.games.replay.open = false;
            state.games.match_history.open = false;
            state.games.run_browser.open = true;
            state.games.run_browser.loading = true;
            state.games.run_browser.last_error = None;
            state.games.run_browser.entries.clear();
            state.games.run_browser.selected = 0;
            state.games.run_browser.scroll_offset = 0;
            state.games.pending_run_browser = true;
            state.status = Some("Games run browser opened".into());
            false
        }
        ["games", "replay"] => {
            if state.games.last_run.is_none() {
                state.status = Some("No run loaded for replay".into());
            } else {
                state.games.run_browser.open = false;
                state.games.match_history.open = false;
                state.games.replay.open = true;
                state.games.replay.loading = false;
                state.games.replay.last_error = None;
                state.games.replay.selected_pair = None;
                state.games.replay.selected_index = 0;
                state.games.replay.title = None;
                state.games.replay.lines.clear();
                state.games.replay.scroll_offset = 0;
                state.games.replay.cycle = None;
                state.status = Some("Games replay opened".into());
            }
            false
        }
        ["games", "history"] | ["games", "hist"] | ["games", "plot"] | ["games", "plots"] => {
            open_games_history_popup(state);
            false
        }
        ["history"] | ["hist"] | ["plot"] | ["plots"] if state.app_kind == AppKind::Games => {
            open_games_history_popup(state);
            false
        }
        _ if tokens.first() == Some(&"games") && tokens.get(1) == Some(&"inspect") => {
            games_inspect::handle_games_inspect(state, &tokens, trimmed);
            false
        }
        ["games", "strategy"]
        | ["games", "strategies"]
        | ["games", "strategy", "run"]
        | ["games", "strategies", "run"] => {
            games_inspect::handle_games_strategy_run(state);
            false
        }
        ["games", "strategy", "all"]
        | ["games", "strategies", "all"]
        | ["games", "strategy", "config"]
        | ["games", "strategies", "config"] => {
            games_inspect::handle_games_strategy_config(state);
            false
        }
        _ if tokens.first() == Some(&"games") && tokens.get(1) == Some(&"tm") => {
            games_inspect::handle_games_tm(state, &tokens, trimmed);
            false
        }
        _ if tokens.first() == Some(&"games") && tokens.get(1) == Some(&"ca") => {
            games_inspect::handle_games_ca(state, &tokens, trimmed);
            false
        }
        _ if tokens.first() == Some(&"games")
            && matches!(tokens.get(1), Some(&"analyze") | Some(&"analyse")) =>
        {
            games_inspect::handle_games_analyze(state, trimmed);
            false
        }
        ["games", "export"] => {
            state.games.pending_export = true;
            false
        }
        ["gol", "seed"] => {
            state.visualizer.seed_view = state.visualizer.seed_view.next();
            state.status = Some(format!("Seed view: {}", state.visualizer.seed_view.label()));
            false
        }
        ["seed", "view"] if state.app_kind == AppKind::Gol => {
            state.visualizer.seed_view = state.visualizer.seed_view.next();
            state.status = Some(format!("Seed view: {}", state.visualizer.seed_view.label()));
            false
        }
        ["gol", "encoder"] => {
            state.visualizer.seed_encoder = state.visualizer.seed_encoder.next();
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Encoder: {}",
                state.visualizer.seed_encoder.label()
            ));
            false
        }
        ["gol", "encoder", name] => {
            if let Some(id) = SeedEncoderId::from_str_name(name) {
                state.visualizer.seed_encoder = id;
                state.visualizer.pending_reseed = true;
                state.status = Some(format!("Encoder: {}", id.label()));
            } else {
                state.status = Some(format!("Unknown encoder: {name}"));
            }
            false
        }
        ["seed", "encoder"] if state.app_kind == AppKind::Gol => {
            state.visualizer.seed_encoder = state.visualizer.seed_encoder.next();
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Encoder: {}",
                state.visualizer.seed_encoder.label()
            ));
            false
        }
        ["seed", "encoder", name] if state.app_kind == AppKind::Gol => {
            if let Some(id) = SeedEncoderId::from_str_name(name) {
                state.visualizer.seed_encoder = id;
                state.visualizer.pending_reseed = true;
                state.status = Some(format!("Encoder: {}", id.label()));
            } else {
                state.status = Some(format!("Unknown encoder: {name}"));
            }
            false
        }
        _ if tokens.first() == Some(&"gol") && tokens.get(1) == Some(&"rule") => {
            if tokens.len() == 2 {
                log_rule_overview(state);
            } else {
                let selector = trimmed
                    .split_whitespace()
                    .skip(2)
                    .collect::<Vec<_>>()
                    .join(" ");
                match state.rule_catalog.select(&selector) {
                    Ok(selected) => apply_rule_selection(state, selected, true),
                    Err(err) => {
                        state.status = Some(format!(
                            "Invalid GoL rule '{selector}': {err}. Try B3/S23 or 'conway'."
                        ));
                    }
                }
            }
            false
        }
        _ if tokens.first() == Some(&"gol") && tokens.get(1) == Some(&"rules") => {
            log_rule_list(state);
            false
        }
        ["petri", "hide"] | ["hide", "petri"] if state.app_kind == AppKind::Gol => {
            state.visualizer.pending_hide = true;
            state.status = Some("Petri dish hiding".into());
            false
        }
        ["petri", "show"] | ["show", "petri"] if state.app_kind == AppKind::Gol => {
            state.visualizer.pending_show = true;
            state.status = Some("Petri dish showing".into());
            false
        }
        other => {
            state.status = Some(format!("Unknown command: {}", other.join(" ")));
            false
        }
    }
}
