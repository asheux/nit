use nit_core::{skills, AppState};

pub(super) fn matches(state: &AppState) -> Vec<usize> {
    let query = state.skill_filter.trim().to_lowercase();
    skills::definitions()
        .iter()
        .enumerate()
        .filter(|(_, skill)| {
            query.is_empty()
                || [skill.title, skill.name, skill.group, skill.summary]
                    .iter()
                    .any(|field| field.to_lowercase().contains(&query))
        })
        .map(|(index, _)| index)
        .collect()
}

pub(super) fn rows(indices: &[usize]) -> impl Iterator<Item = (Option<usize>, &'static str)> + '_ {
    let entries = skills::definitions();
    indices
        .iter()
        .enumerate()
        .flat_map(move |(position, &index)| {
            let entry = &entries[index];
            let heading = (position == 0 || entries[indices[position - 1]].group != entry.group)
                .then_some((None, entry.group));
            heading
                .into_iter()
                .chain(std::iter::once((Some(index), entry.title)))
        })
}

pub(super) fn reconcile(state: &mut AppState) {
    let indices = matches(state);
    if !indices.contains(&state.selected_skill) {
        if let Some(&index) = indices.first() {
            super::select(state, index);
        }
    }
    state.gate_monitor_scroll = 0;
    state.ui_selection = None;
}

pub(super) fn step(state: &mut AppState, amount: isize) {
    let indices = matches(state);
    if !indices.is_empty() {
        let at = indices
            .iter()
            .position(|&index| index == state.selected_skill)
            .unwrap_or(0);
        let next = at.saturating_add_signed(amount).min(indices.len() - 1);
        super::select(state, indices[next]);
    }
}
