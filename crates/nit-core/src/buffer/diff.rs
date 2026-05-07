use ropey::{Rope, RopeSlice};

use super::types::LineDiffStatus;
use super::Buffer;

mod patience;

const FNV_OFFSET: u64 = 14695981039346656037;
const FNV_PRIME: u64 = 1099511628211;

// Match nit_syntax::hash_line_bytes semantics: ignore trailing '\n' and skip '\r'.
// Iterate over rope chunks to avoid allocating a String.
fn fnv1a_line(line: RopeSlice<'_>) -> u64 {
    let mut hash = FNV_OFFSET;
    for chunk in line.chunks() {
        for &b in chunk.as_bytes() {
            if b == b'\n' || b == b'\r' {
                continue;
            }
            hash ^= b as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    hash
}

impl Buffer {
    pub fn line_hash(&self, line: usize) -> u64 {
        if line >= self.rope.len_lines() {
            return FNV_OFFSET;
        }
        fnv1a_line(self.rope.line(line))
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
        self.diff_version = u64::MAX;
    }

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
        self.diff_status = patience::compute_line_diff(&self.base_line_hashes, &current_hashes);
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
    (0..rope.len_lines())
        .map(|i| fnv1a_line(rope.line(i)))
        .collect()
}
