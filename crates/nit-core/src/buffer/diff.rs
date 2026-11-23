use ropey::{Rope, RopeSlice};

use super::types::LineDiffStatus;
use super::Buffer;

mod patience;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReloadOutcome {
    Unchanged,
    Reloaded,
    Conflict,
}

// FNV-1a tuned to match nit_syntax::hash_line_bytes: skip '\n' and '\r' so the
// hash is whitespace-stable across CRLF/LF and trailing-newline differences.
const FNV_OFFSET: u64 = 14695981039346656037;
const FNV_PRIME: u64 = 1099511628211;

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

    /// Diff status for a specific line vs the on-disk base. Lazily recomputes
    /// when the buffer's edit version has advanced past the cached diff.
    pub fn line_diff_status(&mut self, line: usize) -> LineDiffStatus {
        self.ensure_diff_computed();
        self.diff_status
            .get(line)
            .copied()
            .unwrap_or(LineDiffStatus::Unchanged)
    }

    /// Reseed the diff base from a git HEAD blob. Subsequent calls to
    /// `line_diff_status` / `compute_diff_if_needed` recompute against this
    /// content instead of the on-disk file.
    pub fn set_git_base(&mut self, base_content: &str) {
        let base_rope = Rope::from_str(base_content);
        self.base_line_hashes = rope_line_hashes(&base_rope);
        self.invalidate_diff_cache();
    }

    pub fn diff_statuses(&self) -> &[LineDiffStatus] {
        &self.diff_status
    }

    pub fn compute_diff_if_needed(&mut self) {
        self.ensure_diff_computed();
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
        self.saved_rope = self.rope.clone();
        // After save, current content becomes the new base for diff.
        self.base_line_hashes = rope_line_hashes(&self.rope);
        self.invalidate_diff_cache();
        // Pin the current history head as the on-disk anchor so an undo back
        // to this point no longer reports the buffer as dirty.
        self.mark_saved();
    }

    /// Reload the buffer from disk if the file has changed. Drops undo/redo
    /// history and any pending edits — the new content is treated as a fresh
    /// load. Returns `true` if the buffer was updated.
    pub fn reload_from_disk(&mut self) -> bool {
        self.reload_if_clean() == ReloadOutcome::Reloaded
    }

    pub fn reload_if_clean(&mut self) -> ReloadOutcome {
        let Some(path) = self.path.as_ref() else {
            return ReloadOutcome::Unchanged;
        };
        let content = match crate::load_to_string(path) {
            Ok(content) => content,
            Err(crate::io::IoError::Io(err))
                if err.kind() == std::io::ErrorKind::NotFound && self.rope != self.saved_rope =>
            {
                return ReloadOutcome::Conflict;
            }
            Err(_) => return ReloadOutcome::Unchanged,
        };
        // Save notifications can arrive after typing resumes. Compare against
        // the saved rope (a cheap shared snapshot) before declaring a conflict.
        if self.saved_rope == content.as_str() {
            return ReloadOutcome::Unchanged;
        }
        if self.rope != self.saved_rope {
            if self.rope == content.as_str() {
                self.mark_clean();
                return ReloadOutcome::Unchanged;
            }
            return ReloadOutcome::Conflict;
        }
        self.rope = Rope::from_str(&content);
        self.saved_rope = self.rope.clone();
        self.cursor.line = self.clamped_cursor_line();
        self.clamp_col();
        self.cursor.desired_col = None;
        self.clear_selection();
        self.ensure_visible();
        self.record_full_reparse();
        self.undo_log = super::undo_log::UndoLog::new();
        self.dirty = false;
        self.base_line_hashes = rope_line_hashes(&self.rope);
        self.invalidate_diff_cache();
        ReloadOutcome::Reloaded
    }

    pub(super) fn ensure_diff_computed(&mut self) {
        if self.diff_version == self.version {
            return;
        }
        let changed_lines = self.changed_lines.take();
        if self.current_line_hashes.len() != self.rope.len_lines() {
            self.current_line_hashes = rope_line_hashes(&self.rope);
        } else if let Some(lines) = changed_lines {
            for row in lines {
                self.current_line_hashes[row] = fnv1a_line(self.rope.line(row));
            }
        }
        self.diff_status =
            patience::compute_line_diff(&self.base_line_hashes, &self.current_line_hashes);
        self.diff_version = self.version;
    }

    fn invalidate_diff_cache(&mut self) {
        self.diff_version = u64::MAX;
    }
}

pub(super) fn rope_line_hashes(rope: &Rope) -> Vec<u64> {
    rope.lines().map(fnv1a_line).collect()
}
