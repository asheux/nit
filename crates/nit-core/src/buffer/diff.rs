use ropey::Rope;
use std::collections::HashMap;

use super::types::LineDiffStatus;
use super::Buffer;

impl Buffer {
    pub fn line_hash(&self, line: usize) -> u64 {
        const OFFSET: u64 = 14695981039346656037;
        const PRIME: u64 = 1099511628211;
        if line >= self.rope.len_lines() {
            return OFFSET;
        }
        let mut hash = OFFSET;
        // Match nit_syntax::hash_line_bytes semantics: ignore trailing '\n' and skip '\r'.
        // Iterate over rope chunks to avoid allocating a String.
        for chunk in self.rope.line(line).chunks() {
            for &b in chunk.as_bytes() {
                if b == b'\n' || b == b'\r' {
                    continue;
                }
                hash ^= b as u64;
                hash = hash.wrapping_mul(PRIME);
            }
        }
        hash
    }

    /// Get the diff status for a specific line. Lazily recomputes when buffer changes.
    pub fn line_diff_status(&mut self, line: usize) -> LineDiffStatus {
        self.ensure_diff_computed();
        self.diff_status
            .get(line)
            .copied()
            .unwrap_or(LineDiffStatus::Unchanged)
    }

    pub fn has_diff(&mut self) -> bool {
        self.ensure_diff_computed();
        self.diff_status
            .iter()
            .any(|s| *s != LineDiffStatus::Unchanged)
    }

    /// Set the base content for diff computation from a git HEAD blob.
    pub fn set_git_base(&mut self, base_content: &str) {
        let base_rope = Rope::from_str(base_content);
        self.base_line_hashes = rope_line_hashes(&base_rope);
        self.diff_version = u64::MAX; // force recomputation
    }

    /// Cached diff status slice. Call `compute_diff_if_needed` first.
    pub fn diff_statuses(&self) -> &[LineDiffStatus] {
        &self.diff_status
    }

    pub fn compute_diff_if_needed(&mut self) {
        self.ensure_diff_computed();
    }

    pub(super) fn ensure_diff_computed(&mut self) {
        if self.diff_version == self.version {
            return;
        }
        let current_hashes: Vec<u64> = (0..self.rope.len_lines())
            .map(|i| self.line_hash(i))
            .collect();
        self.diff_status = compute_line_diff(&self.base_line_hashes, &current_hashes);
        self.diff_version = self.version;
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
        // After save, current content becomes the new base for diff.
        self.base_line_hashes = rope_line_hashes(&self.rope);
        self.diff_version = u64::MAX;
    }

    /// Reload the buffer content from disk if the file has changed.
    /// Returns `true` if the buffer was updated.
    pub fn reload_from_disk(&mut self) -> bool {
        let Some(path) = self.path.as_ref() else {
            return false;
        };
        let Ok(content) = std::fs::read_to_string(path) else {
            return false;
        };
        let new_rope = Rope::from_str(&content);
        if self.rope == new_rope {
            return false;
        }
        self.rope = new_rope;
        self.version = self.version.wrapping_add(1);
        self.full_reparse = true;
        self.pending_edits.clear();
        self.undo.clear();
        self.redo.clear();
        self.dirty = false;
        self.base_line_hashes = rope_line_hashes(&self.rope);
        self.diff_version = u64::MAX;
        true
    }
}

pub(super) fn rope_line_hashes(rope: &Rope) -> Vec<u64> {
    const OFFSET: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;
    let n = rope.len_lines();
    let mut hashes = Vec::with_capacity(n);
    for i in 0..n {
        let mut hash = OFFSET;
        for chunk in rope.line(i).chunks() {
            for &b in chunk.as_bytes() {
                if b == b'\n' || b == b'\r' {
                    continue;
                }
                hash ^= b as u64;
                hash = hash.wrapping_mul(PRIME);
            }
        }
        hashes.push(hash);
    }
    hashes
}

fn compute_line_diff(base: &[u64], current: &[u64]) -> Vec<LineDiffStatus> {
    if base.is_empty() {
        return vec![LineDiffStatus::Added; current.len()];
    }
    if current.is_empty() {
        return Vec::new();
    }

    // Patience diff: anchor on unique lines, recurse between anchors.
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

/// Convert a mapping (current→base) into per-line diff status.
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
