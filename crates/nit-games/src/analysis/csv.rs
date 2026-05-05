use std::borrow::Cow;
use std::fs::File;
use std::io::{BufWriter, Write};

use super::trajectory::TrajectoryData;
use super::{MatchSummary, StrategySummary};

pub(super) fn write_match_summary(
    writer: &mut BufWriter<File>,
    summary: &MatchSummary,
) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, summary).map_err(|e| e.to_string())?;
    writer.write_all(b"\n").map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn write_csv_header(
    writer: &mut BufWriter<File>,
    columns: &[&str],
) -> Result<(), String> {
    writeln!(writer, "{}", columns.join(",")).map_err(|e| e.to_string())
}

pub(super) const MATCHES_CSV_COLUMNS: &[&str] = &[
    "match_id",
    "match_index",
    "total_matches",
    "repetition",
    "rounds",
    "a",
    "b",
    "a_score",
    "b_score",
    "cc",
    "cd",
    "dc",
    "dd",
    "a_coop_rate",
    "b_coop_rate",
    "tail_rounds",
    "tail_cc",
    "tail_cd",
    "tail_dc",
    "tail_dd",
    "a_tail_coop_rate",
    "b_tail_coop_rate",
    "a_initial",
    "b_initial",
];

pub(super) const TRAJECTORIES_CSV_COLUMNS: &[&str] = &[
    "match_id",
    "match_index",
    "a",
    "b",
    "sample_index",
    "round_start",
    "round_end",
    "window_rounds",
    "a_coop_rate",
    "b_coop_rate",
];

pub(super) fn write_match_csv_row(
    writer: &mut BufWriter<File>,
    summary: &MatchSummary,
) -> Result<(), String> {
    let a = csv_escape(&summary.a);
    let b = csv_escape(&summary.b);
    let a_initial = summary.a_initial.map_or(String::new(), |c| c.to_string());
    let b_initial = summary.b_initial.map_or(String::new(), |c| c.to_string());
    writeln!(
        writer,
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{:.6},{:.6},{},{},{},{},{},{:.6},{:.6},{},{}",
        summary.match_id,
        summary.match_index,
        summary.total_matches,
        summary.repetition,
        summary.rounds,
        a,
        b,
        summary.a_score,
        summary.b_score,
        summary.outcomes.cc,
        summary.outcomes.cd,
        summary.outcomes.dc,
        summary.outcomes.dd,
        summary.a_coop_rate,
        summary.b_coop_rate,
        summary.tail_rounds,
        summary.tail_outcomes.cc,
        summary.tail_outcomes.cd,
        summary.tail_outcomes.dc,
        summary.tail_outcomes.dd,
        summary.a_tail_coop_rate,
        summary.b_tail_coop_rate,
        a_initial,
        b_initial
    )
    .map_err(|e| e.to_string())
}

const STRATEGIES_CSV_COLUMNS: &[&str] = &[
    "id",
    "matches",
    "rounds",
    "coop_rounds",
    "coop_rate",
    "tail_rounds",
    "tail_coop_rounds",
    "tail_coop_rate",
    "total_score",
    "avg_score_per_round",
];

pub(super) fn write_strategies_csv(
    path: &str,
    strategies: &[StrategySummary],
) -> Result<(), String> {
    let mut writer = BufWriter::new(File::create(path).map_err(|e| e.to_string())?);
    write_csv_header(&mut writer, STRATEGIES_CSV_COLUMNS)?;
    for strategy in strategies {
        let id = csv_escape(&strategy.id);
        writeln!(
            writer,
            "{},{},{},{},{:.6},{},{},{:.6},{},{:.6}",
            id,
            strategy.matches,
            strategy.rounds,
            strategy.coop_rounds,
            strategy.coop_rate,
            strategy.tail_rounds,
            strategy.tail_coop_rounds,
            strategy.tail_coop_rate,
            strategy.total_score,
            strategy.avg_score_per_round
        )
        .map_err(|e| e.to_string())?;
    }
    writer.flush().map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn write_trajectory_samples(
    writer: &mut BufWriter<File>,
    match_id: usize,
    match_index: usize,
    a: &str,
    b: &str,
    rounds: u32,
    data: &TrajectoryData,
) -> Result<(), String> {
    let a_id = csv_escape(a);
    let b_id = csv_escape(b);
    for (idx, (a_rate, b_rate)) in data
        .a_rates
        .iter()
        .copied()
        .zip(data.b_rates.iter().copied())
        .enumerate()
    {
        let start = data.starts.get(idx).copied().unwrap_or(1);
        let end = data.ends.get(idx).copied().unwrap_or(rounds);
        let window_rounds = end.saturating_sub(start).saturating_add(1);
        writeln!(
            writer,
            "{},{},{},{},{},{},{},{},{:.6},{:.6}",
            match_id,
            match_index,
            a_id.as_ref(),
            b_id.as_ref(),
            idx,
            start,
            end,
            window_rounds,
            a_rate,
            b_rate
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn csv_escape(value: &str) -> Cow<'_, str> {
    if !value.contains([',', '"', '\n']) {
        return Cow::Borrowed(value);
    }
    let escaped = value.replace('"', "\"\"");
    Cow::Owned(format!("\"{escaped}\""))
}
