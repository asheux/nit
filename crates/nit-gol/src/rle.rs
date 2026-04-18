//! Run-length encoding for Game of Life grids.
//!
//! Implements the standard `.rle` interchange format used by the Life
//! community. Encoding accepts either a [`Grid`] or a packed bitset;
//! both inputs produce byte-identical output.
//!
//! Each row is emitted as a sequence of `<count><tag>` runs (`o` =
//! alive, `b` = dead), rows are separated by `$`, and the document
//! terminates with `!`. These tag bytes are part of the published
//! format spec and must remain stable.

use std::io::{self, Write};

use crate::{Grid, Rule};

const ALIVE_TAG: u8 = b'o';
const DEAD_TAG: u8 = b'b';
const ROW_SEPARATOR: &[u8] = b"$\n";
const TERMINATOR: &[u8] = b"!";
const BITS_PER_WORD: usize = u64::BITS as usize;

/// Encode a grid as a complete RLE document, header included.
#[must_use]
pub fn encode_rle(grid: &Grid, rule: Rule) -> String {
    let mut buf = Vec::new();
    write_rle(&mut buf, grid, rule).expect("writing into Vec<u8> is infallible");
    String::from_utf8(buf).expect("RLE output is always ASCII")
}

/// Stream RLE bytes for `grid` into `writer`.
pub fn write_rle<W: Write>(writer: &mut W, grid: &Grid, rule: Rule) -> io::Result<()> {
    write_rle_document(
        writer,
        grid.width(),
        grid.height(),
        &rule.to_string(),
        |x, y| grid.get(x, y),
    )
}

/// Stream RLE bytes from a packed bitset. Bit `y * width + x` is
/// LSB-first within each `u64` word.
///
/// Returns `InvalidInput` when `bits` is too short for the dimensions,
/// so callers never read past the slice.
pub fn write_rle_bits<W: Write>(
    writer: &mut W,
    width: u16,
    height: u16,
    rule: &str,
    bits: &[u64],
) -> io::Result<()> {
    let cols = width as usize;
    let rows = height as usize;
    ensure_bitset_fits(cols, rows, bits)?;
    write_rle_document(writer, cols, rows, rule, |x, y| bit_at(bits, y * cols + x))
}

fn write_rle_document<W, F>(
    writer: &mut W,
    width: usize,
    height: usize,
    rule: &str,
    mut cell_alive: F,
) -> io::Result<()>
where
    W: Write,
    F: FnMut(usize, usize) -> bool,
{
    writeln!(writer, "x = {width}, y = {height}, rule = {rule}")?;
    // Degenerate grids have no rows to emit; skip the loop so we never
    // write a stray separator between non-existent rows.
    if width == 0 || height == 0 {
        return writer.write_all(TERMINATOR);
    }
    for y in 0..height {
        write_row_runs(writer, width, |x| cell_alive(x, y))?;
        if y + 1 < height {
            writer.write_all(ROW_SEPARATOR)?;
        }
    }
    writer.write_all(TERMINATOR)
}

fn ensure_bitset_fits(width: usize, height: usize, bits: &[u64]) -> io::Result<()> {
    let total_cells = width.saturating_mul(height);
    let needed_words = total_cells.div_ceil(BITS_PER_WORD);
    if bits.len() >= needed_words {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "grid bitset too small: need {needed_words} u64 words for {width}x{height}, got {}",
            bits.len()
        ),
    ))
}

fn write_row_runs<W, F>(writer: &mut W, width: usize, mut cell_alive: F) -> io::Result<()>
where
    W: Write,
    F: FnMut(usize) -> bool,
{
    let mut run_tag = tag_for(cell_alive(0));
    let mut run_len: usize = 1;
    for x in 1..width {
        let tag = tag_for(cell_alive(x));
        if tag == run_tag {
            run_len += 1;
            continue;
        }
        write_run(writer, run_len, run_tag)?;
        run_tag = tag;
        run_len = 1;
    }
    write_run(writer, run_len, run_tag)
}

#[inline]
const fn tag_for(alive: bool) -> u8 {
    if alive {
        ALIVE_TAG
    } else {
        DEAD_TAG
    }
}

/// Emit a single `<count><tag>` run. The count is omitted for
/// singletons, as the `.rle` spec prescribes.
fn write_run<W: Write>(writer: &mut W, len: usize, tag: u8) -> io::Result<()> {
    if len > 1 {
        write!(writer, "{len}")?;
    }
    writer.write_all(&[tag])
}

#[inline]
fn bit_at(bits: &[u64], idx: usize) -> bool {
    let word = bits[idx / BITS_PER_WORD];
    let mask = 1u64 << (idx % BITS_PER_WORD);
    (word & mask) != 0
}
