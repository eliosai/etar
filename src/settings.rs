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
    /// Default uncompressed-to-compressed ratio ceiling
    pub const DEFAULT_MAX_RATIO: u64 = 200;
    /// Output below which the compression-ratio guard stays silent
    pub const RATIO_FLOOR_BYTES: u64 = 8 * 1024 * 1024;
}
