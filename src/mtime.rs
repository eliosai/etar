//! PAX `mtime` values: whole seconds with an optional decimal fraction

use crate::error::Error;
use crate::settings::pax::NANOS_DIGITS;

/// Parse `<secs>` or `<secs>.<fraction>` into seconds and nanoseconds
///
/// Fractions shorter than nine digits are zero-extended and longer ones truncated.
pub fn parse(value: &[u8]) -> Result<(u64, u32), Error> {
    let (secs, fraction) = match value.iter().position(|byte| *byte == b'.') {
        Some(dot) => (&value[..dot], Some(&value[dot + 1..])),
        None => (value, None),
    };
    let secs = digits(secs)?
        .iter()
        .try_fold(0u64, |acc, digit| {
            acc.checked_mul(10)?.checked_add(u64::from(*digit))
        })
        .ok_or(Error::Malformed)?;
    let nanos = match fraction {
        Some(fraction) => nanos(digits(fraction)?),
        None => 0,
    };
    Ok((secs, nanos))
}

/// Scale up to nine fraction digits into nanoseconds, dropping any beyond nine
fn nanos(digits: Vec<u8>) -> u32 {
    (0..NANOS_DIGITS).fold(0, |acc, index| {
        acc * 10 + u32::from(digits.get(index).copied().unwrap_or(0))
    })
}

/// Decimal digit values of a nonempty all-digit field
fn digits(field: &[u8]) -> Result<Vec<u8>, Error> {
    if field.is_empty() || !field.iter().all(u8::is_ascii_digit) {
        return Err(Error::Malformed);
    }
    Ok(field.iter().map(|byte| byte - b'0').collect())
}
