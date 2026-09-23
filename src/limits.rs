//! Resource and safety caps enforced while opening an untrusted archive

/// Default entry-count ceiling
const DEFAULT_MAX_ENTRIES: u64 = 2_000_000;
/// Default total uncompressed byte ceiling
const DEFAULT_MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024 * 1024;
/// Default per-entry uncompressed byte ceiling
const DEFAULT_MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// Default entry-path byte ceiling
const DEFAULT_MAX_PATH_LEN: usize = 4096;
/// Default uncompressed-to-compressed ratio ceiling
const DEFAULT_MAX_RATIO: u64 = 200;

/// Caps an untrusted archive must respect while opening
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SecurityLimits {
    /// Largest number of entries before the archive is rejected
    pub max_entries: u64,
    /// Largest total uncompressed bytes across all entry bodies
    pub max_total_bytes: u64,
    /// Largest uncompressed bytes in one entry body
    pub max_entry_bytes: u64,
    /// Largest entry-path length in bytes
    pub max_path_len: usize,
    /// Largest uncompressed-to-compressed ratio before a bomb is suspected
    pub max_compression_ratio: u64,
    /// Whether symlink entries are surfaced rather than rejected
    pub allow_symlinks: bool,
    /// Whether hardlink entries are surfaced rather than rejected
    pub allow_hardlinks: bool,
}

impl Default for SecurityLimits {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_MAX_ENTRIES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_entry_bytes: DEFAULT_MAX_ENTRY_BYTES,
            max_path_len: DEFAULT_MAX_PATH_LEN,
            max_compression_ratio: DEFAULT_MAX_RATIO,
            allow_symlinks: true,
            allow_hardlinks: true,
        }
    }
}

impl SecurityLimits {
    /// Set the entry-count ceiling
    pub fn with_max_entries(mut self, max: u64) -> Self {
        self.max_entries = max;
        self
    }

    /// Set the total uncompressed byte ceiling
    pub fn with_max_total_bytes(mut self, max: u64) -> Self {
        self.max_total_bytes = max;
        self
    }

    /// Set the per-entry uncompressed byte ceiling
    pub fn with_max_entry_bytes(mut self, max: u64) -> Self {
        self.max_entry_bytes = max;
        self
    }

    /// Set the entry-path byte ceiling
    pub fn with_max_path_len(mut self, max: usize) -> Self {
        self.max_path_len = max;
        self
    }

    /// Set the compression-ratio ceiling
    pub fn with_max_compression_ratio(mut self, max: u64) -> Self {
        self.max_compression_ratio = max;
        self
    }

    /// Set whether symlinks are surfaced
    pub fn with_symlinks(mut self, allow: bool) -> Self {
        self.allow_symlinks = allow;
        self
    }

    /// Set whether hardlinks are surfaced
    pub fn with_hardlinks(mut self, allow: bool) -> Self {
        self.allow_hardlinks = allow;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_defaults_are_conservative() {
        let limits = SecurityLimits::default();
        assert_eq!(limits.max_entries, DEFAULT_MAX_ENTRIES);
        assert!(limits.allow_symlinks);
        assert!(limits.allow_hardlinks);
    }

    #[test]
    fn test_setters_override_defaults() {
        let limits = SecurityLimits::default()
            .with_max_entries(10)
            .with_max_entry_bytes(64)
            .with_symlinks(false)
            .with_hardlinks(false);
        assert_eq!(limits.max_entries, 10);
        assert_eq!(limits.max_entry_bytes, 64);
        assert!(!limits.allow_symlinks);
        assert!(!limits.allow_hardlinks);
    }
}
