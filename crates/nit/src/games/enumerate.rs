use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::Context;
use nit_games::{enumerate_fsms, FsmDefinition, InputMode};

use crate::cli::EnumerateCommand;

pub(super) fn dispatch_enumerate(kind: EnumerateCommand) -> anyhow::Result<()> {
    match kind {
        EnumerateCommand::Fsm {
            states,
            out,
            canonical,
            limit,
            input_mode,
        } => run_games_enumerate_fsm(&states, &out, canonical, limit, input_mode),
    }
}

fn run_games_enumerate_fsm(
    states: &str,
    out: &Path,
    canonical: bool,
    limit: Option<usize>,
    input_mode: Option<String>,
) -> anyhow::Result<()> {
    let range = parse_states_range(states)?;
    let mode = parse_input_mode_arg(input_mode.as_deref())?;

    let out_path = if out.extension().is_some_and(|ext| ext == "ndjson") {
        out.to_path_buf()
    } else {
        fs::create_dir_all(out)?;
        let filename = format!(
            "fsm_enumeration__states-{}.ndjson",
            states.replace("..", "-")
        );
        out.join(filename)
    };

    let mut writer = BufWriter::new(
        fs::File::create(&out_path)
            .with_context(|| format!("failed to create {}", out_path.display()))?,
    );

    let mut total = 0usize;
    for states in range {
        let remaining = limit.and_then(|limit| limit.checked_sub(total));
        if matches!(remaining, Some(0)) {
            break;
        }
        total += enumerate_fsms(states, mode, remaining, canonical, |def: FsmDefinition| {
            let id = format!("fsm_{:016x}", def.stable_hash());
            let spec = def.to_spec(id);
            serde_json::to_writer(&mut writer, &spec).expect("write fsm strategy");
            writer.write_all(b"\n").expect("write newline");
        });
    }

    writer.flush()?;
    eprintln!("FSM enumeration written: {}", out_path.display());
    Ok(())
}

fn parse_states_range(input: &str) -> anyhow::Result<std::ops::RangeInclusive<usize>> {
    let trimmed = input.trim();
    // Try `..=` before `..` to avoid partial match on the inclusive separator.
    let bounds = trimmed
        .split_once("..=")
        .or_else(|| trimmed.split_once(".."));

    if let Some((left, right)) = bounds {
        let start: usize = left.trim().parse()?;
        let end: usize = right.trim().parse()?;
        anyhow::ensure!(start <= end, "states range start must be <= end");
        Ok(start..=end)
    } else {
        let value: usize = trimmed.parse()?;
        Ok(value..=value)
    }
}

fn parse_input_mode_arg(input: Option<&str>) -> anyhow::Result<InputMode> {
    let Some(raw) = input else {
        return Ok(InputMode::OpponentLastAction);
    };
    let normalized: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    match normalized.as_str() {
        "opponentlastaction" | "opponent" | "opp" | "opplastaction" => {
            Ok(InputMode::OpponentLastAction)
        }
        "selflastaction" | "self" | "selflast" => Ok(InputMode::SelfLastAction),
        "jointlastaction" | "joint" | "jointlast" => Ok(InputMode::JointLastAction),
        _ => anyhow::bail!(
            "invalid input_mode '{raw}': expected opponent_last_action, self_last_action, or joint_last_action"
        ),
    }
}
