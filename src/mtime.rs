//! PAX `mtime` values: whole seconds with an optional decimal fraction

use crate::error::Error;
use crate::settings::pax::NANOS_DIGITS;

/// Parse `<secs>[.<fraction>]` into seconds and nanoseconds, dropping digits past nine
pub fn parse(value: &[u8]) -> Result<(u64, u32), Error> {
    let text = std::str::from_utf8(value).map_err(|_| Error::Malformed)?;
    let (secs, fraction) = text.split_once('.').unwrap_or((text, "0"));
    let secs = digits(secs)?.parse().map_err(|_| Error::Malformed)?;
    let nanos = digits(fraction)?
        .bytes()
        .chain(std::iter::repeat(b'0'))
        .take(NANOS_DIGITS)
        .fold(0, |acc, digit| acc * 10 + u32::from(digit - b'0'));
    Ok((secs, nanos))
}

/// A nonempty all-digit field
fn digits(field: &str) -> Result<&str, Error> {
    match !field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit()) {
        true => Ok(field),
        false => Err(Error::Malformed),
    }
}
