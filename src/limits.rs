//! Resource caps enforced while reading an untrusted archive

use crate::settings::open::{
    DEFAULT_MAX_ENTRIES, DEFAULT_MAX_ENTRY_BYTES, DEFAULT_MAX_PATH_LEN, DEFAULT_MAX_TOTAL_BYTES,
    DEFAULT_MAX_XATTR_BYTES,
};

/// Caps an untrusted archive must respect while it is read
///
/// # Example
///
/// ```
/// use etar::ReadLimits;
///
/// let limits = ReadLimits {
///     max_entries: 1_000,
///     ..ReadLimits::default()
/// };
/// assert_eq!(limits.max_entries, 1_000);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadLimits {
    /// Largest number of entries before the archive is rejected
    pub max_entries: u64,
    /// Largest total decoded bytes, tar structure included
    pub max_total_bytes: u64,
    /// Largest bytes in one entry body
    pub max_entry_bytes: u64,
    /// Largest entry-path or link-target length in bytes
    pub max_path_len: usize,
    /// Largest total extended-attribute bytes, names plus values, on one entry
    pub max_xattr_bytes: u64,
}

impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_MAX_ENTRIES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_entry_bytes: DEFAULT_MAX_ENTRY_BYTES,
            max_path_len: DEFAULT_MAX_PATH_LEN,
            max_xattr_bytes: DEFAULT_MAX_XATTR_BYTES,
        }
    }
}
