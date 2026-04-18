//! Run-length encoding (RLE) for Game of Life grids.
//!
//! Implements the standard `.rle` file format used by the Life community
//! for pattern interchange. Supports encoding from both [`Grid`] cell
//! arrays and packed `u64` bitset representations.
//!
//! The RLE format encodes each row as a sequence of run-length pairs
//! (`<count><tag>` where `o` = alive, `b` = dead), rows separated by
//! `$`, and the pattern terminated by `!`.

use std::io::{self, Write};

use crate::{Grid, Rule};

const ALIVE_TAG: u8 = b'o';
const DEAD_TAG: u8 = b'b';
const ROW_SEPARATOR: &[u8] = b"$\n";
const TERMINATOR: &[u8] = b"!";

/// Encode a grid as a complete RLE string.
///
/// Returns the full `.rle` content including the header line with
/// dimensions and rule, followed by run-length encoded cell data.
#[must_use]
pub fn encode_rle(grid: &Grid, rule: Rule) -> String {
    let mut buf = Vec::new();
    write_rle(&mut buf, grid, rule).expect("writing into Vec<u8> is infallible");
    String::from_utf8(buf).expect("RLE output is always ASCII")
}

/// Write RLE-encoded grid data to a generic writer.
///
/// Outputs the standard header (`x = W, y = H, rule = R`) followed
/// by run-length encoded rows separated by `$` and terminated with `!`.
pub fn write_rle<W: Write>(writer: &mut W, grid: &Grid, rule: Rule) -> io::Result<()> {
    write_grid_body(
        writer,
        grid.width(),
        grid.height(),
        &rule.to_string(),
        |x, y| grid.get(x, y),
    )
}

/// Write RLE from a packed bitset representation.
///
/// Accepts grid dimensions and a `u64`-packed bitset where bit `i`
/// of word `i/64` (LSB-first within the word) corresponds to cell
/// index `i = y * width + x`. Returns an error if the bitset is too
/// small for the given dimensions.
pub fn write_rle_bits<W: Write>(
    writer: &mut W,
    width: u16,
    height: u16,
    rule: &str,
    bits: &[u64],
) -> io::Result<()> {
    let w = width as usize;
    let h = height as usize;
    check_bitset_capacity(w, h, bits)?;
    write_grid_body(writer, w, h, rule, |x, y| bit_at(bits, y * w + x))
}

/// Emit the RLE header, per-row runs, and terminator for any cell accessor.
fn write_grid_body<W, F>(
    writer: &mut W,
    width: usize,
    height: usize,
    rule: &str,
    mut get: F,
) -> io::Result<()>
where
    W: Write,
    F: FnMut(usize, usize) -> bool,
{
    write_rle_header(writer, width, height, rule)?;
    if width == 0 || height == 0 {
        return writer.write_all(TERMINATOR);
    }
    for y in 0..height {
        encode_row(writer, width, |x| get(x, y))?;
        write_row_separator(writer, y, height)?;
    }
    writer.write_all(TERMINATOR)
}

fn write_rle_header<W: Write>(
    writer: &mut W,
    width: usize,
    height: usize,
    rule: &str,
) -> io::Result<()> {
    writeln!(writer, "x = {width}, y = {height}, rule = {rule}")
}

fn write_row_separator<W: Write>(writer: &mut W, row: usize, total_rows: usize) -> io::Result<()> {
    if row + 1 < total_rows {
        writer.write_all(ROW_SEPARATOR)
    } else {
        Ok(())
    }
}

fn check_bitset_capacity(width: usize, height: usize, bits: &[u64]) -> io::Result<()> {
    let total = width.saturating_mul(height);
    let needed_words = total.div_ceil(64);
    if bits.len() < needed_words {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "grid bitset too small: need {needed_words} u64 words for {width}x{height}, got {}",
                bits.len()
            ),
        ));
    }
    Ok(())
}

/// Run-length encode a single row by polling cells via a closure.
fn encode_row<W, F>(writer: &mut W, width: usize, mut get: F) -> io::Result<()>
where
    W: Write,
    F: FnMut(usize) -> bool,
{
    let mut run_tag = cell_tag(get(0));
    let mut run_len: usize = 1;
    for x in 1..width {
        let tag = cell_tag(get(x));
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
const fn cell_tag(alive: bool) -> u8 {
    if alive {
        ALIVE_TAG
    } else {
        DEAD_TAG
    }
}

/// Emit a single run-length pair: `<count><tag>` for runs longer than
/// one, or just `<tag>` for singletons, per the `.rle` specification.
fn write_run<W: Write>(writer: &mut W, len: usize, tag: u8) -> io::Result<()> {
    if len > 1 {
        write!(writer, "{len}")?;
    }
    writer.write_all(&[tag])
}

#[inline]
fn bit_at(bits: &[u64], idx: usize) -> bool {
    let word = bits[idx / 64];
    let mask = 1u64 << (idx % 64);
    (word & mask) != 0
}
