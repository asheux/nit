//! Run-length encoding for Game of Life grids.
//!
//! Implements the `.rle` interchange format. Tag bytes (`o` alive,
//! `b` dead), the row separator (`$`), and the terminator (`!`) are
//! part of the published spec and must remain stable.

use std::io::{self, Write};

use crate::{Grid, Rule};

const ALIVE_TAG: u8 = b'o';
const DEAD_TAG: u8 = b'b';
const ROW_SEPARATOR: &[u8] = b"$\n";
const TERMINATOR: &[u8] = b"!";
const BITS_PER_WORD: usize = u64::BITS as usize;

/// Encode a grid as a complete RLE document.
#[must_use]
pub fn encode_rle(grid: &Grid, rule: Rule) -> String {
    let mut buf = Vec::new();
    write_rle(&mut buf, grid, rule).expect("writing into Vec<u8> is infallible");
    String::from_utf8(buf).expect("RLE output is always ASCII")
}

/// Stream the RLE document for `grid` into `writer`.
pub fn write_rle<W: Write>(writer: &mut W, grid: &Grid, rule: Rule) -> io::Result<()> {
    write_rle_document(
        writer,
        grid.width(),
        grid.height(),
        &rule.to_string(),
        |x, y| grid.get(x, y),
    )
}

/// Stream an RLE document from a packed bitset. Bit `y * width + x` is
/// LSB-first within each `u64` word. Errors with `InvalidInput` when
/// `bits` is too short for the declared dimensions.
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
    // Degenerate dimensions emit only the terminator — no rows means no separators.
    if width != 0 && height != 0 {
        for y in 0..height {
            if y > 0 {
                writer.write_all(ROW_SEPARATOR)?;
            }
            write_row_runs(writer, width, |x| cell_alive(x, y))?;
        }
    }
    writer.write_all(TERMINATOR)
}

fn ensure_bitset_fits(width: usize, height: usize, bits: &[u64]) -> io::Result<()> {
    let needed_words = width.saturating_mul(height).div_ceil(BITS_PER_WORD);
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
        let next_tag = tag_for(cell_alive(x));
        if next_tag == run_tag {
            run_len += 1;
            continue;
        }
        write_run(writer, run_len, run_tag)?;
        run_tag = next_tag;
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

/// Emit a single `<count><tag>` run. The count is elided for singletons.
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
