use std::collections::HashMap;

use super::super::types::LineDiffStatus;

pub(super) fn compute_line_diff(base: &[u64], current: &[u64]) -> Vec<LineDiffStatus> {
    if base.is_empty() {
        return vec![LineDiffStatus::Added; current.len()];
    }
    if current.is_empty() {
        return Vec::new();
    }

    let mut mapping: Vec<Option<usize>> = vec![None; current.len()];
    patience_match(base, current, 0, base.len(), 0, current.len(), &mut mapping);
    build_diff_status(base, current, &mapping)
}

/// Patience diff: match equal prefix/suffix, anchor on unique common lines, recurse.
fn patience_match(
    base: &[u64],
    current: &[u64],
    b_lo: usize,
    b_hi: usize,
    c_lo: usize,
    c_hi: usize,
    result: &mut [Option<usize>],
) {
    if b_lo >= b_hi || c_lo >= c_hi {
        return;
    }

    let mut prefix = 0;
    while b_lo + prefix < b_hi
        && c_lo + prefix < c_hi
        && base[b_lo + prefix] == current[c_lo + prefix]
    {
        result[c_lo + prefix] = Some(b_lo + prefix);
        prefix += 1;
    }

    let mut suffix = 0;
    while b_lo + prefix < b_hi.saturating_sub(suffix)
        && c_lo + prefix < c_hi.saturating_sub(suffix)
        && base[b_hi - 1 - suffix] == current[c_hi - 1 - suffix]
    {
        result[c_hi - 1 - suffix] = Some(b_hi - 1 - suffix);
        suffix += 1;
    }

    let b_lo = b_lo + prefix;
    let b_hi = b_hi.saturating_sub(suffix);
    let c_lo = c_lo + prefix;
    let c_hi = c_hi.saturating_sub(suffix);

    if b_lo >= b_hi || c_lo >= c_hi {
        return;
    }

    let mut base_positions: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, &h) in base.iter().enumerate().take(b_hi).skip(b_lo) {
        base_positions.entry(h).or_default().push(i);
    }
    let mut curr_count: HashMap<u64, usize> = HashMap::new();
    for &h in &current[c_lo..c_hi] {
        *curr_count.entry(h).or_default() += 1;
    }

    // Lines unique in both ranges → patience anchors.
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (j, &h) in current.iter().enumerate().take(c_hi).skip(c_lo) {
        if let Some(positions) = base_positions.get(&h) {
            if positions.len() == 1 && curr_count.get(&h) == Some(&1) {
                pairs.push((positions[0], j));
            }
        }
    }

    let anchors = lis_by_first(&pairs);

    if anchors.is_empty() {
        // No unique anchors: fall back to greedy for this segment.
        greedy_match(base, current, b_lo, b_hi, c_lo, c_hi, result);
        return;
    }

    let mut b_prev = b_lo;
    let mut c_prev = c_lo;
    for &(bi, ci) in &anchors {
        patience_match(base, current, b_prev, bi, c_prev, ci, result);
        result[ci] = Some(bi);
        b_prev = bi + 1;
        c_prev = ci + 1;
    }
    patience_match(base, current, b_prev, b_hi, c_prev, c_hi, result);
}

fn greedy_match(
    base: &[u64],
    current: &[u64],
    b_lo: usize,
    b_hi: usize,
    c_lo: usize,
    c_hi: usize,
    result: &mut [Option<usize>],
) {
    let mut i = b_lo;
    let mut j = c_lo;
    let window = (b_hi - b_lo).max(c_hi - c_lo).min(50);
    while i < b_hi && j < c_hi {
        if base[i] == current[j] {
            result[j] = Some(i);
            i += 1;
            j += 1;
            continue;
        }
        let mut next_i = None;
        for di in 1..=window {
            let idx = i + di;
            if idx >= b_hi {
                break;
            }
            if base[idx] == current[j] {
                next_i = Some(idx);
                break;
            }
        }
        let mut next_j = None;
        for dj in 1..=window {
            let idx = j + dj;
            if idx >= c_hi {
                break;
            }
            if current[idx] == base[i] {
                next_j = Some(idx);
                break;
            }
        }
        match (next_i, next_j) {
            (Some(ni), Some(nj)) => {
                if ni - i <= nj - j {
                    i = ni;
                } else {
                    j = nj;
                }
            }
            (Some(ni), None) => i = ni,
            (None, Some(nj)) => j = nj,
            (None, None) => j += 1,
        }
    }
}

/// Longest increasing subsequence on (base_idx, current_idx) pairs, ordered by base_idx.
fn lis_by_first(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    if pairs.is_empty() {
        return Vec::new();
    }
    let mut tails: Vec<usize> = Vec::new();
    let mut predecessors: Vec<Option<usize>> = vec![None; pairs.len()];

    for (idx, &(bi, _)) in pairs.iter().enumerate() {
        let pos = tails.partition_point(|&t| pairs[t].0 < bi);
        if pos > 0 {
            predecessors[idx] = Some(tails[pos - 1]);
        }
        if pos == tails.len() {
            tails.push(idx);
        } else {
            tails[pos] = idx;
        }
    }

    let mut result = Vec::new();
    let mut cur = tails.last().copied();
    while let Some(idx) = cur {
        result.push(pairs[idx]);
        cur = predecessors[idx];
    }
    result.reverse();
    result
}

fn build_diff_status(
    base: &[u64],
    current: &[u64],
    mapping: &[Option<usize>],
) -> Vec<LineDiffStatus> {
    let mut status = vec![LineDiffStatus::Added; current.len()];
    for (j, m) in mapping.iter().enumerate() {
        if m.is_some() {
            status[j] = LineDiffStatus::Unchanged;
        }
    }

    let mut cj = 0;
    while cj < current.len() {
        if status[cj] != LineDiffStatus::Added {
            cj += 1;
            continue;
        }
        let region_start = cj;
        while cj < current.len() && status[cj] == LineDiffStatus::Added {
            cj += 1;
        }
        let current_unmatched = cj - region_start;

        let base_start = if region_start > 0 {
            mapping[region_start - 1].map(|bi| bi + 1).unwrap_or(0)
        } else {
            0
        };
        let base_end = if cj < current.len() {
            mapping[cj].unwrap_or(base.len())
        } else {
            base.len()
        };
        let base_unmatched = base_end.saturating_sub(base_start);

        let modified_count = current_unmatched.min(base_unmatched);
        for k in 0..modified_count {
            status[region_start + k] = LineDiffStatus::Modified;
        }

        if base_unmatched > current_unmatched
            && cj < status.len()
            && status[cj] == LineDiffStatus::Unchanged
        {
            status[cj] = LineDiffStatus::DeletedAbove;
        }
    }

    status
}
