//! Pure numeric helpers for strategy index encoding and decoding.

/// Returns `(q, r)` such that `numer == q * denom + r` and `0 <= r < denom`.
pub(crate) fn floor_div_rem_i128(numer: i128, denom: i128) -> (i128, i128) {
    (numer.div_euclid(denom), numer.rem_euclid(denom))
}

/// Absolute-value digits of a signed integer, MSD first, zero-padded to `len`.
pub(crate) fn integer_digits_signed_abs(value: i128, radix: usize, len: usize) -> Vec<usize> {
    integer_digits_unsigned(value.unsigned_abs(), radix, len)
}

/// MSD-first base-`radix` digits, zero-padded to `len`. Radix clamped to >= 2.
pub(crate) fn integer_digits_unsigned(mut value: u128, radix: usize, len: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let radix_u128 = radix.max(2) as u128;
    let mut digits = vec![0usize; len];
    for slot in digits.iter_mut().rev() {
        *slot = (value % radix_u128) as usize;
        value /= radix_u128;
    }
    digits
}

macro_rules! define_checked_pow {
    ($name:ident, $val_ty:ty, $exp_ty:ty) => {
        pub(crate) fn $name(base: $val_ty, exponent: $exp_ty) -> Option<$val_ty> {
            let mut result: $val_ty = 1;
            for _ in 0..exponent {
                result = result.checked_mul(base)?;
            }
            Some(result)
        }
    };
}

define_checked_pow!(checked_pow_u128, u128, u32);
define_checked_pow!(checked_pow_usize, usize, usize);

/// MSD-first digits of `input` in the given base. Returns `[0]` for zero.
pub(crate) fn digits_in_base(input: u64, base: u8) -> Vec<u8> {
    let radix = base.max(2) as u64;
    if input == 0 {
        return vec![0];
    }
    let mut remaining = input;
    let mut digits = Vec::new();
    while remaining > 0 {
        digits.push((remaining % radix) as u8);
        remaining /= radix;
    }
    digits.reverse();
    digits
}

/// Reconstruct a `u64` from MSD-first digits in the given base.
/// Returns `None` on overflow.
pub(crate) fn digits_to_u64(digits: &[u8], base: u8) -> Option<u64> {
    let radix = base.max(2) as u64;
    let mut accumulated = 0u64;
    for &digit in digits {
        accumulated = accumulated.checked_mul(radix)?;
        accumulated = accumulated.checked_add(digit as u64)?;
    }
    Some(accumulated)
}
