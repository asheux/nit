//! Shared numeric helpers for strategy encoding and decoding.
//!
//! All functions in this module are pure, deterministic, and operate on
//! primitive integer types. They support the encoding and decoding of
//! strategy indices across FSM, CA, and TM strategy types.

/// Floor division with non-negative remainder for signed 128-bit integers.
///
/// Unlike Rust's built-in truncating division, this always rounds the
/// quotient toward negative infinity, ensuring the remainder is
/// non-negative. Returns `(q, r)` such that `numer == q * denom + r`
/// and `0 <= r < denom`.
pub(crate) fn floor_div_rem_i128(numer: i128, denom: i128) -> (i128, i128) {
    let mut q = numer / denom;
    let mut r = numer % denom;
    if r < 0 {
        q -= 1;
        r += denom;
    }
    (q, r)
}

/// Decompose a signed integer (by absolute value) into base-`base` digits,
/// most-significant digit first, zero-padded to `len`.
pub(crate) fn integer_digits_signed_abs(value: i128, base: usize, len: usize) -> Vec<usize> {
    integer_digits_unsigned(value.unsigned_abs(), base, len)
}

/// Decompose an unsigned 128-bit integer into `len` base-`base` digits,
/// most-significant digit first.
///
/// Returns an empty vector when `len == 0`. Clamps `base` to a minimum of 2.
pub(crate) fn integer_digits_unsigned(mut value: u128, base: usize, len: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let base_u128 = base.max(2) as u128;
    let mut digits = vec![0usize; len];
    for idx in (0..len).rev() {
        digits[idx] = (value % base_u128) as usize;
        value /= base_u128;
    }
    digits
}

/// Checked exponentiation: returns `base.pow(exp)` or `None` on overflow.
pub(crate) fn checked_pow_u128(base: u128, exp: u32) -> Option<u128> {
    let mut value = 1u128;
    for _ in 0..exp {
        value = value.checked_mul(base)?;
    }
    Some(value)
}

/// Checked exponentiation for `usize`: returns `base.pow(exponent)` or `None` on overflow.
pub(crate) fn checked_pow_usize(base: usize, exponent: usize) -> Option<usize> {
    let mut value = 1usize;
    for _ in 0..exponent {
        value = value.checked_mul(base)?;
    }
    Some(value)
}

/// Convert a `u64` to its digit representation in the given base,
/// most-significant digit first.
///
/// The base is clamped to a minimum of 2. Returns `[0]` for zero input.
pub(crate) fn digits_in_base(input: u64, base: u8) -> Vec<u8> {
    let base = base.max(2) as u64;
    if input == 0 {
        return vec![0];
    }
    let mut value = input;
    let mut digits = Vec::new();
    while value > 0 {
        digits.push((value % base) as u8);
        value /= base;
    }
    digits.reverse();
    digits
}

/// Reconstruct a `u64` from a sequence of digits in the given base,
/// most-significant digit first.
///
/// The base is clamped to a minimum of 2. Returns `None` if the
/// accumulated value would overflow `u64`.
pub(crate) fn digits_to_u64(digits: &[u8], base: u8) -> Option<u64> {
    let base_u64 = base.max(2) as u64;
    let mut value = 0u64;
    for &digit in digits {
        value = value.checked_mul(base_u64)?;
        value = value.checked_add(digit as u64)?;
    }
    Some(value)
}
