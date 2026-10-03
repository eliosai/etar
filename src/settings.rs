//! Compile-time archive settings

pub mod compression {
    /// Bytes peeked to detect compression
    pub const PEEK_LEN: usize = 4;
    /// zstd frame magic
    pub const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
    /// Buffered-reader capacity feeding the decoder
    pub const BUF_CAP: usize = 64 * 1024;
}

pub mod open {
    /// Default entry-count ceiling
    pub const DEFAULT_MAX_ENTRIES: u64 = 2_000_000;
    /// Default total uncompressed byte ceiling
    pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024 * 1024;
    /// Default per-entry uncompressed byte ceiling
    pub const DEFAULT_MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024 * 1024;
    /// Default entry-path byte ceiling
    pub const DEFAULT_MAX_PATH_LEN: usize = 4096;
    /// Default maximum size of a GNU or PAX metadata record
    pub const DEFAULT_MAX_METADATA_BYTES: u64 = 1024 * 1024;
    /// Default total extended-attribute bytes (names plus values) per entry
    pub const DEFAULT_MAX_XATTR_BYTES: u64 = 64 * 1024;
    /// Default uncompressed-to-compressed ratio ceiling
    pub const DEFAULT_MAX_RATIO: u64 = 200;
    /// Output below which the compression-ratio guard stays silent
    pub const RATIO_FLOOR_BYTES: u64 = 8 * 1024 * 1024;
}

pub mod zstd {
    /// Slowest-to-largest level zstd accepts for output (fastest negative level)
    pub const MIN_LEVEL: i32 = -131_072;
    /// Strongest level zstd accepts
    pub const MAX_LEVEL: i32 = 22;
    /// Level zstd uses when the caller chooses none
    pub const DEFAULT_LEVEL: i32 = 3;
    /// Most compression worker threads zstd supports
    pub const MAX_WORKERS: u32 = 256;
}

pub mod pax {
    /// PAX key prefix GNU tar and star use for an extended attribute
    pub const SCHILY_XATTR: &[u8] = b"SCHILY.xattr.";
    /// PAX key prefix libarchive uses, with a percent-encoded name and base64 value
    pub const LIBARCHIVE_XATTR: &[u8] = b"LIBARCHIVE.xattr.";
    /// Path of the PAX header entry preceding an entry
    pub const HEADER_PATH: &str = "././@PaxHeader";
}
