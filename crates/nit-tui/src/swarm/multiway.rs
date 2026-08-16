//! `@swarm … mode=multiway` → [`SearchPolicy`]/[`Task`] mapping, beside the
//! [`SwarmCommand`](super::SwarmCommand) it reads. int-route strips the
//! `mode=multiway` token before `parse_swarm_command`, so this only maps an
//! already-parsed command. v1 runs the prompt as the search task verbatim.

use crate::multiway::runtime::clamp_fork_width;
use crate::multiway::{Mood, SearchPolicy, Task};

use super::types::{parse_swarm_template, SwarmMissionKind, SwarmSize, SwarmTemplate};
use super::{effective_max_swarm_size, SwarmCommand};

/// Fork fan-out for a requested swarm size, clamped into the FD-bounded ceiling
/// (the idempotent bound `MultiwayRuntime::start` re-applies) so a wide request
/// can't exhaust file descriptors.
fn fork_width(size: SwarmSize) -> usize {
    let ceiling = effective_max_swarm_size();
    let requested = match size {
        SwarmSize::Default => SearchPolicy::default().k,
        SwarmSize::All => ceiling,
        SwarmSize::Count(explicit) => explicit,
    };
    clamp_fork_width(requested, ceiling)
}

/// The fan-out templates search breadth-first; lab converges on one integrator.
fn search_mood(template: SwarmTemplate) -> Mood {
    match template {
        SwarmTemplate::Parallel | SwarmTemplate::Bulk => Mood::Explore,
        SwarmTemplate::Lab => Mood::Balanced,
    }
}

/// Research kinds carry their role through; general or unspecified missions run
/// as the default `integrate` writer, matching the other multiway front-ends.
fn task_role(mission: Option<SwarmMissionKind>) -> &'static str {
    match mission {
        None | Some(SwarmMissionKind::General) => "integrate",
        Some(kind) => kind.label(),
    }
}

pub fn swarm_multiway_policy(cmd: &SwarmCommand) -> (SearchPolicy, Task) {
    let policy = SearchPolicy {
        mood: search_mood(parse_swarm_template(cmd.template.as_deref())),
        k: fork_width(cmd.size),
        budget: SearchPolicy::default().budget,
    };
    let task = Task {
        prompt: cmd.prompt.clone(),
        role: task_role(cmd.mission_kind).to_owned(),
    };
    (policy, task)
}
