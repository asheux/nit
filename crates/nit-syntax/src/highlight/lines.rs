//! Byte-offset line-start table plus a line-ending-agnostic FNV-1a hash used
//! to detect which lines have changed between snapshots.

/// Returns line-start byte offsets with a trailing sentinel at `text.len()`
/// so every line index `i` maps to the half-open range `[offsets[i], offsets[i + 1])`.
pub(crate) fn compute_line_starts(text: &str) -> Vec<usize> {
    let mut offsets = vec![0usize];
    offsets.extend(
        text.bytes()
            .enumerate()
            .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
    );

    let last = *offsets.last().unwrap_or(&0);
    if last != text.len() {
        offsets.push(text.len());
    }
    offsets
}

pub(crate) fn find_line(offsets: &[usize], target_byte: usize) -> usize {
    offsets
        .partition_point(|&boundary| boundary <= target_byte)
        .saturating_sub(1)
}

/// FNV-1a over `raw` with `\n`/`\r` stripped so `"a\n"`, `"a\r\n"`, and `"a"`
/// hash identically. Callers may treat `0` as an "unhashed" sentinel: a real
/// FNV-1a output equal to `0` is astronomically unlikely (≈1/2⁶⁴) and would
/// just trigger a redundant re-highlight, never silent corruption.
#[must_use]
pub fn hash_line_bytes(raw: &[u8]) -> u64 {
    const BASIS: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;

    let end = if raw.last() == Some(&b'\n') {
        raw.len() - 1
    } else {
        raw.len()
    };

    raw[..end]
        .iter()
        .filter(|&&b| b != b'\r')
        .fold(BASIS, |hash, &b| (hash ^ b as u64).wrapping_mul(PRIME))
}

pub(crate) fn recompute_line_hashes(
    text: &[u8],
    line_starts: &[usize],
    hashes: &mut [u64],
    lines: impl Iterator<Item = usize>,
) {
    for i in lines {
        if i + 1 < line_starts.len() && i < hashes.len() {
            hashes[i] = hash_line_bytes(&text[line_starts[i]..line_starts[i + 1]]);
        }
    }
}
