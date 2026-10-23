//! Extended attribute checks and PAX decoding under a byte budget

use crate::entry::Meta;
use crate::error::Error;
use crate::settings::pax::SCHILY_XATTR;
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
