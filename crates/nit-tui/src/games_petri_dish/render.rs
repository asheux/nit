use std::time::Duration;

use nit_core::{AppState, GamesStatus};
use nit_games::output::StrategyDefinition;
use nit_games::{MatchSnapshot, TournamentProgress};
use nit_metal::BatchPolicyCacheSnapshot;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::theme::Theme;
use crate::widgets::games_visualizer_view::strategy_display_name_from_def;

pub(super) fn normalize_path(input: &str) -> String {
    let trimmed = input.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|v| v.strip_suffix('\''))
        })
        .unwrap_or(trimmed);
    unquoted.trim().to_string()
}

pub(crate) fn progress_waiting_text(status: GamesStatus) -> &'static str {
    match status {
        GamesStatus::Idle => "Waiting for tournament...",
        GamesStatus::Running => "Starting tournament...",
        GamesStatus::Paused => "Paused before first round",
        GamesStatus::Done => "Tournament complete",
        GamesStatus::Error => "Tournament unavailable",
    }
}

pub(crate) fn progress_pending_round_text(status: GamesStatus) -> &'static str {
    match status {
        GamesStatus::Running => "Round pending...",
        GamesStatus::Paused => "Paused",
        _ => progress_waiting_text(status),
    }
}

pub(super) fn format_tournament_elapsed(duration: Duration) -> String {
    if duration.as_secs() == 0 {
        return format!("{}ms", duration.as_millis());
    }
    let total_secs = duration.as_secs();
    let millis = duration.subsec_millis();
    let hours = total_secs / 3_600;
    let minutes = (total_secs / 60) % 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m{seconds:02}.{millis:03}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}.{millis:03}s")
    } else {
        format!("{seconds}.{millis:03}s")
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn session_footer_line(
    steps_per_tick: u32,
    steps_use_match_units: bool,
    paused: bool,
    elapsed: Duration,
    label_style: Style,
    number_style: Style,
    paused_style: Style,
    dim_style: Style,
) -> Line<'static> {
    let speed_label = if steps_use_match_units {
        "matches/tick: "
    } else {
        "steps/tick: "
    };
    Line::from(vec![
        Span::styled(speed_label, label_style),
        Span::styled(steps_per_tick.to_string(), number_style),
        Span::styled("  ", dim_style),
        Span::styled("paused: ", label_style),
        Span::styled(if paused { "yes" } else { "no" }, paused_style),
        Span::styled("  ", dim_style),
        Span::styled("elapsed: ", label_style),
        Span::styled(format_tournament_elapsed(elapsed), number_style),
    ])
}

pub(crate) fn tournament_progress_percent(progress: &TournamentProgress) -> f32 {
    if progress.total_matches == 0 || progress.rounds == 0 {
        return 0.0;
    }
    let completed_matches = progress.match_index.saturating_sub(1) as u128;
    let round = progress.round.min(progress.rounds) as u128;
    let rounds_per_match = progress.rounds as u128;
    let total_rounds = (progress.total_matches as u128).saturating_mul(rounds_per_match);
    if total_rounds == 0 {
        return 0.0;
    }
    let done_rounds = completed_matches
        .saturating_mul(rounds_per_match)
        .saturating_add(round);
    ((done_rounds as f64 / total_rounds as f64) * 100.0) as f32
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_progress(
    progress: Option<TournamentProgress>,
    definitions: &[StrategyDefinition],
    state: &AppState,
    label_style: Style,
    value_style: Style,
    number_style: Style,
    dim_style: Style,
    status_style: Style,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let waiting_text = progress_waiting_text(state.games.status);
    let pending_round_text = progress_pending_round_text(state.games.status);
    lines.push(Line::from(vec![
        Span::styled("Status: ", label_style),
        Span::styled(format!("{:?}", state.games.status), status_style),
    ]));
    if let Some(progress) = progress {
        if progress.total_matches == 0 {
            lines.push(Line::from(vec![
                Span::styled("Match: ", label_style),
                Span::styled("0/0", number_style),
                Span::styled(" (no matches scheduled)", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Pair: ", label_style),
                Span::styled("n/a", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Last: ", label_style),
                Span::styled("n/a", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Halt: ", label_style),
                Span::styled("n/a", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Payoff: ", label_style),
                Span::styled("n/a", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Total: ", label_style),
                Span::styled("0", number_style),
                Span::styled(" / ", dim_style),
                Span::styled("0", number_style),
            ]));
            lines.push(accelerator_progress_line(
                &progress.runtime,
                label_style,
                value_style,
                number_style,
                dim_style,
            ));
            if let Some(note) = accelerator_note_line(&progress.runtime, label_style, dim_style) {
                lines.push(note);
            }
            if let Some(cache) = accelerator_cache_line(&progress.runtime, label_style, dim_style) {
                lines.push(cache);
            }
            return lines;
        }
        let a_label = strategy_label_for_pair(&progress.a, definitions);
        let b_label = strategy_label_for_pair(&progress.b, definitions);
        let pct = tournament_progress_percent(&progress);
        let running_completed_snapshot =
            progress.match_complete && progress.match_index < progress.total_matches;
        let mut match_spans = vec![
            Span::styled("Match: ", label_style),
            Span::styled(progress.match_index.to_string(), number_style),
            Span::styled("/", dim_style),
            Span::styled(progress.total_matches.to_string(), number_style),
            Span::styled(" (", dim_style),
        ];
        if running_completed_snapshot {
            match_spans.push(Span::styled("last complete, overall ", dim_style));
        } else {
            match_spans.push(Span::styled("round ", dim_style));
            match_spans.push(Span::styled(progress.round.to_string(), number_style));
            match_spans.push(Span::styled("/", dim_style));
            match_spans.push(Span::styled(progress.rounds.to_string(), number_style));
            match_spans.push(Span::styled(", overall ", dim_style));
        }
        match_spans.push(Span::styled(format!("{pct:>5.1}%"), number_style));
        match_spans.push(Span::styled(")", dim_style));
        lines.push(Line::from(match_spans));
        if running_completed_snapshot {
            let live_copy = if uses_metal_batching(&progress.runtime) {
                "GPU batching; showing last completed match snapshot"
            } else {
                "Showing last completed match snapshot"
            };
            lines.push(Line::from(vec![
                Span::styled("Live: ", label_style),
                Span::styled(live_copy, dim_style),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("Pair: ", label_style),
            Span::styled(a_label, value_style),
            Span::styled(" vs ", dim_style),
            Span::styled(b_label, value_style),
        ]));
        lines.push(Line::from(
            match (progress.last_action_a, progress.last_action_b) {
                (Some(a), Some(b)) => vec![
                    Span::styled("Last: ", label_style),
                    Span::styled(a.as_char().to_string(), number_style),
                    Span::styled(" / ", dim_style),
                    Span::styled(b.as_char().to_string(), number_style),
                ],
                _ => vec![
                    Span::styled("Last: ", label_style),
                    Span::styled(pending_round_text, dim_style),
                ],
            },
        ));
        lines.push(Line::from(
            match (progress.last_halted_a, progress.last_halted_b) {
                (Some(a), Some(b)) => vec![
                    Span::styled("Halt: ", label_style),
                    Span::styled(if a { "1" } else { "0" }, number_style),
                    Span::styled(" / ", dim_style),
                    Span::styled(if b { "1" } else { "0" }, number_style),
                    Span::styled(" ", dim_style),
                    Span::styled("(1=halt, 0=timeout)", dim_style),
                ],
                _ => vec![
                    Span::styled("Halt: ", label_style),
                    Span::styled(pending_round_text, dim_style),
                ],
            },
        ));
        if let (Some(pa), Some(pb)) = (progress.last_payoff_a, progress.last_payoff_b) {
            lines.push(Line::from(vec![
                Span::styled("Payoff: ", label_style),
                Span::styled(pa.to_string(), number_style),
                Span::styled(" / ", dim_style),
                Span::styled(pb.to_string(), number_style),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled("Payoff: ", label_style),
                Span::styled(pending_round_text, dim_style),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("Total: ", label_style),
            Span::styled(progress.total_payoff_a.to_string(), number_style),
            Span::styled(" / ", dim_style),
            Span::styled(progress.total_payoff_b.to_string(), number_style),
        ]));
        lines.push(accelerator_progress_line(
            &progress.runtime,
            label_style,
            value_style,
            number_style,
            dim_style,
        ));
        if let Some(note) = accelerator_note_line(&progress.runtime, label_style, dim_style) {
            lines.push(note);
        }
        if let Some(cache) = accelerator_cache_line(&progress.runtime, label_style, dim_style) {
            lines.push(cache);
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("Match: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Pair: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Last: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Halt: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Payoff: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Total: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(accelerator_progress_line(
            &state.games.runtime,
            label_style,
            value_style,
            number_style,
            dim_style,
        ));
        if let Some(note) = accelerator_note_line(&state.games.runtime, label_style, dim_style) {
            lines.push(note);
        }
        if let Some(cache) = accelerator_cache_line(&state.games.runtime, label_style, dim_style) {
            lines.push(cache);
        }
    }
    lines
}

fn accelerator_progress_line(
    runtime: &nit_games::RuntimeAcceleratorStats,
    label_style: Style,
    value_style: Style,
    number_style: Style,
    dim_style: Style,
) -> Line<'static> {
    let backend = match runtime.backend {
        nit_games::RuntimeAcceleratorBackend::Metal => "metal",
        nit_games::RuntimeAcceleratorBackend::Cpu => "cpu",
        nit_games::RuntimeAcceleratorBackend::None => match runtime.requested {
            nit_games::AcceleratorMode::Cpu => "cpu",
            nit_games::AcceleratorMode::Metal => "metal",
            nit_games::AcceleratorMode::Auto => "auto",
        },
    };
    let mut spans = vec![
        Span::styled("Accel: ", label_style),
        Span::styled(backend.to_ascii_uppercase(), value_style),
    ];
    if runtime.metal_matches > 0 {
        spans.push(Span::styled(" ", dim_style));
        spans.push(Span::styled(
            format!("gpu {}", runtime.metal_matches),
            number_style,
        ));
    }
    if runtime.cpu_matches > 0 {
        spans.push(Span::styled(" ", dim_style));
        spans.push(Span::styled(
            format!("cpu {}", runtime.cpu_matches),
            number_style,
        ));
    }
    if runtime.metal_fallbacks > 0 {
        spans.push(Span::styled(" ", dim_style));
        spans.push(Span::styled(
            format!("fallback {}", runtime.metal_fallbacks),
            dim_style,
        ));
    }
    if let (Some(batch), Some(inflight)) = (
        runtime.metal_matches_per_batch,
        runtime.metal_inflight_batches,
    ) {
        spans.push(Span::styled(" ", dim_style));
        let policy_label = runtime
            .metal_policy_source_label()
            .map(|source| format!("policy {batch}x{inflight} {source}"))
            .unwrap_or_else(|| format!("policy {batch}x{inflight}"));
        spans.push(Span::styled(policy_label, dim_style));
    }
    Line::from(spans)
}

fn uses_metal_batching(runtime: &nit_games::RuntimeAcceleratorStats) -> bool {
    matches!(runtime.backend, nit_games::RuntimeAcceleratorBackend::Metal)
        || runtime.metal_matches > 0
}

fn accelerator_note_line(
    runtime: &nit_games::RuntimeAcceleratorStats,
    label_style: Style,
    dim_style: Style,
) -> Option<Line<'static>> {
    let reason = runtime.metal_fallback_reason.as_ref()?;
    Some(Line::from(vec![
        Span::styled("AccelNote: ", label_style),
        Span::styled(reason.clone(), dim_style),
    ]))
}

fn accelerator_cache_line(
    runtime: &nit_games::RuntimeAcceleratorStats,
    label_style: Style,
    dim_style: Style,
) -> Option<Line<'static>> {
    let value = runtime
        .metal_policy_cache_path
        .as_ref()
        .cloned()
        .or_else(|| runtime.metal_policy_cache_key.as_ref().cloned())?;
    Some(Line::from(vec![
        Span::styled("AccelCache: ", label_style),
        Span::styled(value, dim_style),
    ]))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_cache_browser(
    snapshot: &BatchPolicyCacheSnapshot,
    selected: usize,
    width: usize,
    header_style: Style,
    label_style: Style,
    value_style: Style,
    dim_style: Style,
    key_style: Style,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled("Metal Cache", header_style))];
    lines.push(Line::from(vec![
        Span::styled("root: ", label_style),
        Span::styled(
            truncate_text(
                &snapshot
                    .root
                    .clone()
                    .unwrap_or_else(|| "unavailable".to_string()),
                width.saturating_sub(6),
            ),
            dim_style,
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("entries: ", label_style),
        Span::styled(snapshot.entries.len().to_string(), value_style),
    ]));
    lines.push(Line::from(""));

    if snapshot.entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "No Metal cache entries.",
            dim_style,
        )));
        return lines;
    }

    let selected = selected.min(snapshot.entries.len().saturating_sub(1));
    let visible = 8usize.min(snapshot.entries.len());
    let start = selected
        .saturating_sub(visible / 2)
        .min(snapshot.entries.len() - visible);
    let end = start + visible;
    let row_width = width.saturating_sub(4).max(16);
    for (idx, entry) in snapshot.entries[start..end].iter().enumerate() {
        let absolute_idx = start + idx;
        let marker = if absolute_idx == selected { ">" } else { " " };
        let marker_style = if absolute_idx == selected {
            key_style
        } else {
            dim_style
        };
        let summary = format!(
            "{:>2}. {} {}x{}",
            absolute_idx + 1,
            entry.key,
            entry.matches_per_batch,
            entry.inflight_batches
        );
        lines.push(Line::from(vec![
            Span::styled(marker, marker_style),
            Span::styled(" ", dim_style),
            Span::styled(truncate_text(&summary, row_width), value_style),
        ]));
    }

    let selected_entry = &snapshot.entries[selected];
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("key: ", label_style),
        Span::styled(selected_entry.key.clone(), value_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled("payload: ", label_style),
        Span::styled(
            truncate_text(&selected_entry.payload_signature, width.saturating_sub(9)),
            dim_style,
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("path: ", label_style),
        Span::styled(
            truncate_text(&selected_entry.path, width.saturating_sub(6)),
            dim_style,
        ),
    ]));
    lines
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_match_inspector(
    snapshot: Option<MatchSnapshot>,
    progress: Option<TournamentProgress>,
    definitions: &[StrategyDefinition],
    status: GamesStatus,
    window: usize,
    width: usize,
    header_style: Style,
    label_style: Style,
    value_style: Style,
    number_style: Style,
    dim_style: Style,
    warn_style: Style,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let waiting_text = progress_waiting_text(status);
    let title = if snapshot.is_some() {
        "Live Match"
    } else if progress.as_ref().is_some_and(|progress| {
        progress.match_complete && progress.match_index < progress.total_matches
    }) {
        "Last Completed Match"
    } else {
        "Match Inspector"
    };
    lines.push(Line::from(Span::styled(title, header_style)));

    if let Some(snapshot) = snapshot {
        lines.push(Line::from(vec![
            Span::styled("window: ", label_style),
            Span::styled(window.to_string(), number_style),
            Span::styled(" rounds", dim_style),
        ]));
        let a_label = strategy_label_for_pair(&snapshot.a, definitions);
        let b_label = strategy_label_for_pair(&snapshot.b, definitions);
        lines.push(Line::from(vec![
            Span::styled("Match: ", label_style),
            Span::styled(snapshot.match_index.to_string(), number_style),
            Span::styled("/", dim_style),
            Span::styled(snapshot.total_matches.to_string(), number_style),
            Span::styled(" (round ", dim_style),
            Span::styled(snapshot.round.to_string(), number_style),
            Span::styled("/", dim_style),
            Span::styled(snapshot.rounds.to_string(), number_style),
            Span::styled(")", dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Pair: ", label_style),
            Span::styled(a_label, value_style),
            Span::styled(" vs ", dim_style),
            Span::styled(b_label, value_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Score: ", label_style),
            Span::styled(snapshot.a_score.to_string(), number_style),
            Span::styled(" / ", dim_style),
            Span::styled(snapshot.b_score.to_string(), number_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Outcomes: ", label_style),
            Span::styled("0=CC 1=CD 2=DC 3=DD", dim_style),
        ]));
        lines.extend(render_match_strip(
            &snapshot,
            window,
            width,
            label_style,
            value_style,
            number_style,
            dim_style,
            warn_style,
        ));
    } else if let Some(progress) = progress {
        let a_label = strategy_label_for_pair(&progress.a, definitions);
        let b_label = strategy_label_for_pair(&progress.b, definitions);
        let running_completed_snapshot =
            progress.match_complete && progress.match_index < progress.total_matches;
        let match_detail = if running_completed_snapshot {
            "last complete".to_string()
        } else if progress.round > 0 {
            format!("round {}/{}", progress.round, progress.rounds)
        } else {
            waiting_text.to_string()
        };
        let last_detail = match (
            progress.last_action_a,
            progress.last_action_b,
            progress.last_payoff_a,
            progress.last_payoff_b,
        ) {
            (Some(a), Some(b), Some(payoff_a), Some(payoff_b)) => {
                format!(
                    "{} / {} ({payoff_a} / {payoff_b})",
                    a.as_char(),
                    b.as_char()
                )
            }
            _ => waiting_text.to_string(),
        };
        let halt_detail = match (progress.last_halted_a, progress.last_halted_b) {
            (Some(a), Some(b)) => format!("{} / {} (1=halt, 0=timeout)", a as u8, b as u8),
            _ => waiting_text.to_string(),
        };
        let halt_waiting = halt_detail == waiting_text;
        let note = if running_completed_snapshot {
            if uses_metal_batching(&progress.runtime) {
                "Detailed round history is unavailable during GPU batching; showing the last completed match summary."
            } else {
                "Detailed round history is unavailable; showing the last completed match summary."
            }
        } else {
            "Waiting for a live per-round match snapshot."
        };
        lines.push(Line::from(vec![
            Span::styled("Match: ", label_style),
            Span::styled(progress.match_index.to_string(), number_style),
            Span::styled("/", dim_style),
            Span::styled(progress.total_matches.to_string(), number_style),
            Span::styled(" (", dim_style),
            Span::styled(match_detail, dim_style),
            Span::styled(")", dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Pair: ", label_style),
            Span::styled(a_label, value_style),
            Span::styled(" vs ", dim_style),
            Span::styled(b_label, value_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Total: ", label_style),
            Span::styled(progress.total_payoff_a.to_string(), number_style),
            Span::styled(" / ", dim_style),
            Span::styled(progress.total_payoff_b.to_string(), number_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Last: ", label_style),
            Span::styled(last_detail, value_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Halt: ", label_style),
            Span::styled(
                halt_detail,
                if halt_waiting { dim_style } else { warn_style },
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Note: ", label_style),
            Span::styled(truncate_text(note, width.saturating_sub(6)), dim_style),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("Match: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Pair: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Score: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Outcomes: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
    }
    lines
}

fn strategy_label_for_pair(id: &str, definitions: &[StrategyDefinition]) -> String {
    let Some(def) = definitions.iter().find(|def| def.id == id) else {
        return id.to_string();
    };
    match &def.kind {
        nit_games::config::StrategySpecKind::Fsm {
            num_states,
            outputs,
            transitions,
            index,
            ..
        } => {
            let states = if outputs.is_empty() {
                *num_states
            } else {
                outputs.len()
            };
            let k = transitions.first().map(|row| row.len()).unwrap_or(2);
            if let Some(index) = index {
                format!("{id} {{n={index}, s={states}, k={k}}}")
            } else {
                format!("{id} {{s={states}, k={k}}}")
            }
        }
        nit_games::config::StrategySpecKind::Ca { n, k, r, t } => {
            let _ = t;
            format!("{id} {{n={n}, k={k}, r={r}}}")
        }
        nit_games::config::StrategySpecKind::OneSidedTm {
            rule_code,
            states,
            symbols,
            ..
        } => {
            if let Some(rule) = rule_code {
                format!("{id} {{n={rule}, s={states}, k={symbols}}}")
            } else {
                format!("{id} {{s={states}, k={symbols}}}")
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_match_strip(
    snapshot: &MatchSnapshot,
    window: usize,
    width: usize,
    label_style: Style,
    value_style: Style,
    number_style: Style,
    dim_style: Style,
    warn_style: Style,
) -> Vec<Line<'static>> {
    let total = snapshot.outcomes.len().min(snapshot.payoffs.len());
    if total == 0 || window == 0 {
        return vec![Line::from(vec![
            Span::styled("  ", dim_style),
            Span::styled("--", dim_style),
        ])];
    }
    let mut cumulative = Vec::with_capacity(total);
    let mut a_total = 0i64;
    let mut b_total = 0i64;
    for payoff in snapshot.payoffs.iter().take(total) {
        a_total += payoff[0] as i64;
        b_total += payoff[1] as i64;
        cumulative.push((a_total, b_total));
    }

    let halt_token = |index: usize| -> String {
        let a = snapshot.a_halted.as_bytes().get(index).copied();
        let b = snapshot.b_halted.as_bytes().get(index).copied();
        match (a, b) {
            (Some(a), Some(b)) => format!("{}/{}", a as char, b as char),
            _ => "--".to_string(),
        }
    };

    let label_w = 3usize;
    let prefix_len = label_w + 2;
    let available = width.saturating_sub(prefix_len);
    let mut max_len = 3usize;
    let window_start = total.saturating_sub(window);
    for (i, ((&idx_byte, payoff), cumulative)) in snapshot
        .outcomes
        .as_bytes()
        .iter()
        .take(total)
        .zip(snapshot.payoffs.iter().take(total))
        .zip(cumulative.iter())
        .enumerate()
        .skip(window_start)
    {
        let round_len = (i + 1).to_string().chars().count();
        let idx_char = idx_byte as char;
        let out_len = match idx_char {
            '0' | '1' | '2' | '3' => 3,
            _ => 2,
        };
        let payoff_len = format!("{}/{}", payoff[0], payoff[1]).chars().count();
        let total_len = format!("{}/{}", cumulative.0, cumulative.1).chars().count();
        let halt_len = halt_token(i).chars().count();
        max_len = max_len
            .max(round_len)
            .max(out_len)
            .max(payoff_len)
            .max(total_len)
            .max(halt_len);
    }
    let col_w = (max_len + 1).max(4);
    let max_cols = (available / col_w).max(1);
    let visible = window.min(total).min(max_cols);
    let start = total.saturating_sub(visible);

    let fit_right = |value: &str| -> String {
        if col_w == 0 {
            return String::new();
        }
        let len = value.chars().count();
        let trimmed: String = if len > col_w - 1 {
            value.chars().skip(len.saturating_sub(col_w - 1)).collect()
        } else {
            value.to_string()
        };
        format!("{:>width$} ", trimmed, width = col_w - 1)
    };
    let mut idx_line = String::new();
    let mut out_spans = Vec::new();
    let mut halt_spans = Vec::new();
    let mut pay_line = String::new();
    let mut total_line = String::new();
    out_spans.push(Span::styled(
        format!("{:>label_w$}: ", "Out", label_w = label_w),
        label_style,
    ));
    halt_spans.push(Span::styled(
        format!("{:>label_w$}: ", "Hlt", label_w = label_w),
        label_style,
    ));
    for (i, ((&idx_byte, payoff), cumulative)) in snapshot
        .outcomes
        .as_bytes()
        .iter()
        .take(total)
        .zip(snapshot.payoffs.iter().take(total))
        .zip(cumulative.iter())
        .enumerate()
        .skip(start)
    {
        idx_line.push_str(&fit_right(&(i + 1).to_string()));
        let idx_char = idx_byte as char;
        let outcome = match idx_char {
            '0' => "C/C",
            '1' => "C/D",
            '2' => "D/C",
            '3' => "D/D",
            _ => "--",
        };
        let outcome_style = match idx_char {
            '0' => number_style,
            '1' => value_style,
            '2' => warn_style,
            '3' => dim_style,
            _ => dim_style,
        };
        out_spans.push(Span::styled(fit_right(outcome), outcome_style));
        let halt = halt_token(i);
        let halt_style = match (
            snapshot.a_halted.as_bytes().get(i).copied(),
            snapshot.b_halted.as_bytes().get(i).copied(),
        ) {
            (Some(b'1'), Some(b'1')) => dim_style,
            (Some(_), Some(_)) => warn_style,
            _ => dim_style,
        };
        halt_spans.push(Span::styled(fit_right(&halt), halt_style));
        pay_line.push_str(&fit_right(&format!("{}/{}", payoff[0], payoff[1])));
        total_line.push_str(&fit_right(&format!("{}/{}", cumulative.0, cumulative.1)));
    }

    let separator = "-".repeat(width.min(prefix_len + visible * col_w));
    let legend = Line::from(vec![
        Span::styled("Legend: ", label_style),
        Span::styled("CC", number_style),
        Span::styled(" ", dim_style),
        Span::styled("CD", value_style),
        Span::styled(" ", dim_style),
        Span::styled("DC", warn_style),
        Span::styled(" ", dim_style),
        Span::styled("DD", dim_style),
        Span::styled(" | Hlt: 1=halt 0=timeout", dim_style),
    ]);

    vec![
        legend,
        Line::from(vec![
            Span::styled(
                format!("{:>label_w$}: ", "Idx", label_w = label_w),
                label_style,
            ),
            Span::styled(idx_line, number_style),
        ]),
        Line::from(out_spans),
        Line::from(halt_spans),
        Line::from(vec![
            Span::styled(
                format!("{:>label_w$}: ", "Pay", label_w = label_w),
                label_style,
            ),
            Span::styled(pay_line, number_style),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{:>label_w$}: ", "Tot", label_w = label_w),
                label_style,
            ),
            Span::styled(total_line, number_style),
        ]),
        Line::from(Span::styled(separator, dim_style)),
    ]
}

pub(super) fn status_style(state: &AppState, theme: &Theme) -> Style {
    match state.games.status {
        GamesStatus::Idle => Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        GamesStatus::Running => Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
        GamesStatus::Paused => Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD),
        GamesStatus::Done => Style::default().fg(theme.title),
        GamesStatus::Error => Style::default()
            .fg(theme.error)
            .add_modifier(Modifier::BOLD),
    }
}

pub(super) fn lines_to_strings(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

pub(super) fn top_table_widths(
    config: &nit_games::NormalizedConfig,
) -> (usize, usize, usize, usize) {
    let n = config.strategies.len().max(1);
    let matches_per = if config.self_play {
        n.saturating_mul(2)
    } else {
        n.saturating_sub(1).saturating_mul(2)
    };
    let matches_per = matches_per.saturating_mul(config.repetitions.max(1) as usize);
    let rounds = config.rounds.max(1) as i64;
    let mut max_payoff = i32::MIN;
    let mut min_payoff = i32::MAX;
    for row in config.payoff.matrix.iter() {
        for cell in row.iter() {
            for value in cell.iter() {
                max_payoff = max_payoff.max(*value);
                min_payoff = min_payoff.min(*value);
            }
        }
    }
    let max_payoff = max_payoff as i64;
    let min_payoff = min_payoff as i64;
    let max_abs = max_payoff.abs().max(min_payoff.abs());
    let score_header = score_column_label(config);
    let total_header = total_payoff_column_label(config);
    let score_bound = match config.engine.score_aggregation {
        nit_games::ScoreAggregation::Mean => max_abs as f64,
        nit_games::ScoreAggregation::Total => max_abs
            .saturating_mul(matches_per as i64)
            .saturating_mul(rounds) as f64,
    };
    let total_bound = match config.engine.score_aggregation {
        nit_games::ScoreAggregation::Mean => max_abs.saturating_mul(matches_per as i64) as f64,
        nit_games::ScoreAggregation::Total => max_abs
            .saturating_mul(matches_per as i64)
            .saturating_mul(rounds) as f64,
    };
    let score_w = if min_payoff < 0 {
        nit_games::output::format_score_value(-score_bound).len()
    } else {
        nit_games::output::format_score_value(score_bound).len()
    }
    .max(score_header.len());
    let total_w = if min_payoff < 0 {
        nit_games::output::format_score_value(-total_bound).len()
    } else {
        nit_games::output::format_score_value(total_bound).len()
    }
    .max(total_header.len());
    let rank_w = n.to_string().len();
    let wld_w = format!("W{matches_per}-L{matches_per}-D{matches_per}").len();
    (rank_w, score_w, total_w, wld_w)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_top_table(
    results: &nit_games::output::TournamentResults,
    config: &nit_games::NormalizedConfig,
    definitions: &[nit_games::output::StrategyDefinition],
    width: usize,
    header_style: Style,
    label_style: Style,
    value_style: Style,
    number_style: Style,
    win_style: Style,
    loss_style: Style,
    draw_style: Style,
    dim_style: Style,
    fixed_rank_w: usize,
    fixed_score_w: usize,
    fixed_total_w: usize,
    fixed_wld_w: usize,
) -> Vec<Line<'static>> {
    const TOP_LIMIT: usize = 15;
    type Row = (
        String,
        String,
        String,
        String,
        String,
        String,
        u32,
        u32,
        u32,
    );
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled("Top Strategies", header_style)));
    if definitions.is_empty() {
        lines.push(Line::from(Span::styled(
            "Loading strategy definitions...",
            dim_style,
        )));
        return lines;
    }
    if results.ranking.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for leaderboard results...",
            dim_style,
        )));
        return lines;
    }

    let score_header = score_column_label(config);
    let total_header = total_payoff_column_label(config);
    let rows: Vec<Row> = results
        .ranking
        .iter()
        .take(TOP_LIMIT)
        .enumerate()
        .map(|(idx, entry)| {
            let found = definitions.iter().find(|def| def.id == entry.id).cloned();
            let display = found
                .as_ref()
                .map(strategy_display_name_from_def)
                .unwrap_or_else(|| entry.id.clone());
            let machine_n = found
                .as_ref()
                .and_then(strategy_machine_index)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let rank = format!("{}", idx + 1);
            let id = entry.id.clone();
            let score = entry.formatted_score(
                config.engine.score_aggregation,
                config.engine.complexity_cost.enabled,
            );
            let total = entry.formatted_total_payoff(
                config.engine.score_aggregation,
                config.engine.complexity_cost.enabled,
            );
            (
                rank,
                id,
                machine_n,
                display,
                score,
                total,
                entry.wins,
                entry.losses,
                entry.draws,
            )
        })
        .collect();

    let headers = [
        "#",
        "id",
        "n",
        "Strategy",
        score_header,
        total_header,
        "W-L-D",
    ];
    let mut rank_w = headers[0].len().max(fixed_rank_w);
    let mut id_w = headers[1].len();
    let mut n_w = headers[2].len();
    let mut name_w = headers[3].len();
    let mut score_w = headers[4].len().max(fixed_score_w);
    let mut total_w = headers[5].len().max(fixed_total_w);
    let mut wld_w = headers[6].len().max(fixed_wld_w);

    for (rank, id, machine_n, name, score, total, wins, losses, draws) in &rows {
        rank_w = rank_w.max(rank.len());
        id_w = id_w.max(id.len());
        n_w = n_w.max(machine_n.len());
        name_w = name_w.max(name.chars().count());
        score_w = score_w.max(score.len());
        total_w = total_w.max(total.len());
        let wld_len = format!("W{wins}-L{losses}-D{draws}").len();
        wld_w = wld_w.max(wld_len);
    }

    let min_id = 4usize;
    let min_name = 10usize;
    let columns = headers.len();
    let overhead = (columns + 1) + (2 * columns);
    let fixed = rank_w + n_w + score_w + total_w + wld_w;
    let available = width.saturating_sub(overhead + fixed);
    if available >= min_id + min_name {
        id_w = id_w.min(available.saturating_sub(min_name));
        name_w = name_w.min(available.saturating_sub(id_w));
    } else {
        id_w = id_w.min(available.saturating_sub(1).max(1));
        name_w = available.saturating_sub(id_w).max(1);
    }

    let sep = format!(
        "+{}+{}+{}+{}+{}+{}+{}+",
        "-".repeat(rank_w + 2),
        "-".repeat(id_w + 2),
        "-".repeat(n_w + 2),
        "-".repeat(name_w + 2),
        "-".repeat(score_w + 2),
        "-".repeat(total_w + 2),
        "-".repeat(wld_w + 2)
    );
    lines.push(Line::from(Span::styled(sep.clone(), dim_style)));
    lines.push(Line::from(vec![
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(headers[0], rank_w)),
            header_style,
        ),
        Span::styled("|", dim_style),
        Span::styled(format!(" {} ", center_text(headers[1], id_w)), header_style),
        Span::styled("|", dim_style),
        Span::styled(format!(" {} ", center_text(headers[2], n_w)), header_style),
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(headers[3], name_w)),
            header_style,
        ),
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(headers[4], score_w)),
            header_style,
        ),
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(headers[5], total_w)),
            header_style,
        ),
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(headers[6], wld_w)),
            header_style,
        ),
        Span::styled("|", dim_style),
    ]));
    lines.push(Line::from(Span::styled(sep.clone(), dim_style)));

    for (rank, id, machine_n, name, score, total, wins, losses, draws) in rows {
        let mut spans = Vec::new();
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(format!(" {rank:>rank_w$} "), number_style));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(
            format!(" {:<id_w$} ", truncate_text(&id, id_w)),
            label_style,
        ));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(
            format!(" {:>n_w$} ", truncate_text(&machine_n, n_w)),
            if machine_n == "-" {
                dim_style
            } else {
                number_style
            },
        ));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(
            format!(" {:<name_w$} ", truncate_text(&name, name_w)),
            value_style,
        ));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(format!(" {score:>score_w$} "), number_style));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(format!(" {total:>total_w$} "), number_style));
        spans.push(Span::styled("|", dim_style));
        spans.extend(wld_cell_spans(
            wins,
            losses,
            draws,
            wld_w,
            label_style,
            win_style,
            loss_style,
            draw_style,
            dim_style,
        ));
        spans.push(Span::styled("|", dim_style));
        lines.push(Line::from(spans));
    }

    if results.ranking.len() > TOP_LIMIT {
        lines.push(Line::from(vec![
            Span::styled("… showing top ", dim_style),
            Span::styled(TOP_LIMIT.to_string(), number_style),
            Span::styled(" of ", dim_style),
            Span::styled(results.ranking.len().to_string(), number_style),
            Span::styled(" strategies", dim_style),
        ]));
    }

    lines.push(Line::from(Span::styled(sep, dim_style)));
    lines
}

fn score_column_label(config: &nit_games::NormalizedConfig) -> &'static str {
    match config.engine.score_aggregation {
        nit_games::ScoreAggregation::Mean => "Score(mean)",
        nit_games::ScoreAggregation::Total => "Score(total)",
    }
}

fn total_payoff_column_label(config: &nit_games::NormalizedConfig) -> &'static str {
    match config.engine.score_aggregation {
        nit_games::ScoreAggregation::Mean => "AggPayoff",
        nit_games::ScoreAggregation::Total => "TotalPayoff",
    }
}

fn strategy_machine_index(def: &StrategyDefinition) -> Option<u64> {
    match &def.kind {
        nit_games::config::StrategySpecKind::Fsm { index, .. } => *index,
        nit_games::config::StrategySpecKind::Ca { n, .. } => Some(*n),
        nit_games::config::StrategySpecKind::OneSidedTm { rule_code, .. } => *rule_code,
    }
}

#[allow(clippy::too_many_arguments)]
fn wld_cell_spans(
    wins: u32,
    losses: u32,
    draws: u32,
    width: usize,
    label_style: Style,
    win_style: Style,
    loss_style: Style,
    draw_style: Style,
    dim_style: Style,
) -> Vec<Span<'static>> {
    let base = format!("W{wins}-L{losses}-D{draws}");
    let pad = width.saturating_sub(base.len());
    let mut spans = Vec::new();
    spans.push(Span::styled(" ", dim_style));
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), dim_style));
    }
    spans.push(Span::styled("W", label_style));
    spans.push(Span::styled(wins.to_string(), win_style));
    spans.push(Span::styled("-L", label_style));
    spans.push(Span::styled(losses.to_string(), loss_style));
    spans.push(Span::styled("-D", label_style));
    spans.push(Span::styled(draws.to_string(), draw_style));
    spans.push(Span::styled(" ", dim_style));
    spans
}

fn center_text(value: &str, width: usize) -> String {
    let len = value.chars().count();
    if len >= width {
        return truncate_text(value, width);
    }
    let pad = width - len;
    let left = pad / 2;
    let right = pad - left;
    format!("{}{}{}", " ".repeat(left), value, " ".repeat(right))
}

fn truncate_text(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let len = value.chars().count();
    if len <= width {
        return value.to_string();
    }
    if width <= 3 {
        return value.chars().take(width).collect();
    }
    let mut out: String = value.chars().take(width - 3).collect();
    out.push_str("...");
    out
}
