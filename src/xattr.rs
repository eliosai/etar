//! Extended attribute checks and PAX decoding under a byte budget

use crate::entry::Meta;
use crate::error::Error;
use crate::settings::pax::{LIBARCHIVE_XATTR, SCHILY_XATTR};
use std::collections::BTreeMap;

/// Reject attributes the SCHILY encoding cannot carry back
pub fn check(meta: &Meta) -> Result<(), Error> {
    for name in meta.xattrs.keys() {
        if name.is_empty() || name.iter().any(|byte| matches!(byte, 0 | b'=')) {
            return Err(Error::InvalidXattr);
        }
    }
    Ok(())
}

/// Gathers one entry's attributes from PAX records, bounded by a byte budget
pub struct Collector {
    xattrs: BTreeMap<Vec<u8>, Vec<u8>>,
    used: u64,
    max: u64,
}

impl Collector {
    /// Start an empty collection that refuses more than `max` name and value bytes
    pub fn new(max: u64) -> Self {
        Self {
            xattrs: BTreeMap::new(),
            used: 0,
            max,
        }
    }

    /// Take a record when its key is an xattr key and ignore every other key
    pub fn record(&mut self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        if let Some(name) = key.strip_prefix(SCHILY_XATTR) {
            return self.insert(name.to_vec(), value.to_vec());
        }
        if let Some(encoded) = key.strip_prefix(LIBARCHIVE_XATTR) {
            return self.insert(percent_decode(encoded)?, base64_decode(value)?);
        }
        Ok(())
    }

    /// The attributes collected so far
    pub fn finish(self) -> BTreeMap<Vec<u8>, Vec<u8>> {
        self.xattrs
    }

    fn insert(&mut self, name: Vec<u8>, value: Vec<u8>) -> Result<(), Error> {
        if name.is_empty() || name.contains(&0) {
            return Err(Error::Malformed);
        }
        self.used = self.used.saturating_add((name.len() + value.len()) as u64);
        if self.used > self.max {
            return Err(Error::LimitExceeded("max_xattr_bytes"));
        }
        self.xattrs.insert(name, value);
        Ok(())
    }
}

/// Decode `%XX` escapes, passing other bytes through
fn percent_decode(encoded: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(encoded.len());
    let mut rest = encoded;
    while let Some((&byte, tail)) = rest.split_first() {
        rest = tail;
        if byte != b'%' {
            out.push(byte);
            continue;
        }
        let (high, low) = match rest {
            [high, low, ..] => (hex(*high)?, hex(*low)?),
            _ => return Err(Error::Malformed),
        };
        out.push(high << 4 | low);
        rest = &rest[2..];
    }
    Ok(out)
}

fn hex(digit: u8) -> Result<u8, Error> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        b'A'..=b'F' => Ok(digit - b'A' + 10),
        _ => Err(Error::Malformed),
    }
}

/// Decode padded standard base64
fn base64_decode(encoded: &[u8]) -> Result<Vec<u8>, Error> {
    if encoded.is_empty() || !encoded.len().is_multiple_of(4) {
        return Err(Error::Malformed);
    }
    let mut out = Vec::with_capacity(encoded.len() / 4 * 3);
    let (quads, _) = encoded.as_chunks::<4>();
    let last = quads.len() - 1;
    for (index, quad) in quads.iter().enumerate() {
        let padding = quad.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && index != last) {
            return Err(Error::Malformed);
        }
        let mut bits = 0u32;
        for byte in &quad[..4 - padding] {
            bits = bits << 6 | sextet(*byte)?;
        }
        bits <<= 6 * padding as u32;
        out.extend_from_slice(&bits.to_be_bytes()[1..4 - padding]);
    }
    Ok(out)
}

fn sextet(byte: u8) -> Result<u32, Error> {
    match byte {
        b'A'..=b'Z' => Ok(u32::from(byte - b'A')),
        b'a'..=b'z' => Ok(u32::from(byte - b'a') + 26),
        b'0'..=b'9' => Ok(u32::from(byte - b'0') + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(Error::Malformed),
    }
}
