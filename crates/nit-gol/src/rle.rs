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

/// Encode a grid as a complete RLE string.
///
/// Returns the full `.rle` content including the header line with
/// dimensions and rule, followed by run-length encoded cell data.
pub fn encode_rle(grid: &Grid, rule: Rule) -> String {
    let mut buf = Vec::new();
    let _ = write_rle(&mut buf, grid, rule);
    String::from_utf8(buf).unwrap_or_default()
}

/// Write RLE-encoded grid data to a generic writer.
///
/// Outputs the standard header (`x = W, y = H, rule = R`) followed
/// by run-length encoded rows separated by `$` and terminated with `!`.
pub fn write_rle<W: Write>(writer: &mut W, grid: &Grid, rule: Rule) -> io::Result<()> {
    write_rle_header(writer, grid.width(), grid.height(), &rule.to_string())?;
    if grid.width() == 0 || grid.height() == 0 {
        writer.write_all(b"!")?;
        return Ok(());
    }
    for y in 0..grid.height() {
        encode_row(writer, grid.width(), |x| grid.get(x, y))?;
        write_row_separator(writer, y, grid.height())?;
    }
    writer.write_all(b"!")?;
    Ok(())
}

/// Write RLE from a packed bitset representation.
///
/// Accepts grid dimensions and a `u64`-packed bitset where bit `i`
/// of word `i/64` corresponds to cell index `i` in row-major order.
/// Returns an error if the bitset is too small for the given dimensions.
pub fn write_rle_bits<W: Write>(
    writer: &mut W,
    width: u16,
    height: u16,
    rule: &str,
    bits: &[u64],
) -> io::Result<()> {
    let w = width as usize;
    let h = height as usize;
    validate_bitset_size(w, h, bits)?;
    write_rle_header(writer, w, h, rule)?;
    if w == 0 || h == 0 {
        writer.write_all(b"!")?;
        return Ok(());
    }
    for y in 0..h {
        let base = y * w;
        encode_row(writer, w, |x| bit_at(bits, base + x))?;
        write_row_separator(writer, y, h)?;
    }
    writer.write_all(b"!")?;
    Ok(())
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
        writer.write_all(b"$\n")
    } else {
        Ok(())
    }
}

fn validate_bitset_size(width: usize, height: usize, bits: &[u64]) -> io::Result<()> {
    let total = width.saturating_mul(height);
    let needed_words = total.div_ceil(64);
    if bits.len() < needed_words {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "grid bitset too small",
        ));
    }
    Ok(())
}

/// Run-length encode a single row by polling cells via a closure.
fn encode_row<W: Write, F: FnMut(usize) -> bool>(
    writer: &mut W,
    width: usize,
    mut get: F,
) -> io::Result<()> {
    let mut run_tag = cell_tag(get(0));
    let mut run_len = 1usize;
    for x in 1..width {
        let tag = cell_tag(get(x));
        if tag == run_tag {
            run_len += 1;
        } else {
            write_run(writer, run_len, run_tag)?;
            run_tag = tag;
            run_len = 1;
        }
    }
    write_run(writer, run_len, run_tag)
}

fn cell_tag(alive: bool) -> u8 {
    if alive {
        b'o'
    } else {
        b'b'
    }
}

/// Emit a single run-length encoded pair: `<count><tag>` for runs longer
/// than 1, or just `<tag>` for singletons, per the `.rle` specification.
fn write_run<W: Write>(writer: &mut W, len: usize, tag: u8) -> io::Result<()> {
    if len > 1 {
        write!(writer, "{len}")?;
    }
    writer.write_all(&[tag])
}

fn bit_at(bits: &[u64], idx: usize) -> bool {
    let word = bits[idx / 64];
    let mask = 1u64 << (idx % 64);
    (word & mask) != 0
}
