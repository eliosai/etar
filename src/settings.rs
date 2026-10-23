//! Compile-time archive settings

pub mod compression {
    /// Bytes peeked to detect compression
    pub const PEEK_LEN: usize = 4;
    /// zstd frame magic
    pub const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
    /// Buffered-reader capacity feeding the decoder
    pub const BUF_CAP: usize = 64 * 1024;
    /// zstd level the writer compresses at
    pub const LEVEL: i32 = 3;
    /// Largest zstd window the decoder accepts, as a power of two (16 MiB)
    pub const WINDOW_LOG_MAX: u32 = 24;
}

pub mod open {
    /// Default entry-count ceiling
    pub const DEFAULT_MAX_ENTRIES: u64 = 2_000_000;
    /// Default total decoded byte ceiling
    pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024 * 1024;
    /// Default per-entry byte ceiling
    pub const DEFAULT_MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024 * 1024;
    /// Default entry-path byte ceiling
    pub const DEFAULT_MAX_PATH_LEN: usize = 4096;
    /// Default total extended-attribute bytes (names plus values) per entry
    pub const DEFAULT_MAX_XATTR_BYTES: u64 = 64 * 1024;
    /// Largest GNU or PAX metadata record the guard lets the parser buffer
    pub const MAX_METADATA_BYTES: u64 = 1024 * 1024;
}

pub mod device {
    /// Largest major or minor number a seven-digit octal header field holds
    pub const MAX_NUMBER: u32 = 0o7_777_777;
}

pub mod pax {
    /// PAX key prefix GNU tar and star use for an extended attribute
    pub const SCHILY_XATTR: &[u8] = b"SCHILY.xattr.";
    /// PAX key prefix marking a GNU sparse member
    pub const GNU_SPARSE: &[u8] = b"GNU.sparse.";
    /// PAX key holding a modification time with an optional fraction
    pub const MTIME: &str = "mtime";
    /// Nanoseconds in one second
    pub const NANOS_PER_SEC: u32 = 1_000_000_000;
    /// Fraction digits in a nanosecond count
    pub const NANOS_DIGITS: usize = 9;
    /// Path of the PAX header entry preceding an entry
    pub const HEADER_PATH: &str = "././@PaxHeader";
    /// PAX key holding a link target too long for the ustar header
    pub const LINKPATH: &str = "linkpath";
    /// Bytes the ustar linkname field holds
    pub const USTAR_LINK_LEN: usize = 100;
}

#[cfg(feature = "erofs")]
pub mod erofs {
    /// The only mkfs.erofs version an image build accepts
    pub const VERSION: &str = "1.9.4";
    /// Fixed mkfs.erofs flags ahead of the image path: tar on stdin, zstd 64 KiB clusters,
    /// clear UUID, zero timestamps, two workers
    pub const ARGS: [&str; 8] = [
        "--quiet",
        "--tar=f",
        "-zzstd,level=3",
        "-C65536",
        "--workers=2",
        "-Uclear",
        "-T0",
        "--mkfs-time",
    ];
    /// Bytes buffered ahead of the mkfs.erofs stdin pipe
    pub const PIPE_CAP: usize = 64 * 1024;
    /// Bytes of mkfs.erofs stderr kept for an error
    pub const STDERR_TAIL: usize = 4096;
}
