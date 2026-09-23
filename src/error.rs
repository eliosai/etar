//! Errors from path validation, reading, and writing

use std::io;

/// Why an etar operation failed
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The path is absolute or carries a drive prefix
    #[error("absolute path")]
    AbsolutePath,
    /// The path ascends out of its root
    #[error("parent escape")]
    ParentEscape,
    /// The path is empty
    #[error("empty path")]
    EmptyPath,
    /// The path contains a NUL byte
    #[error("nul byte in path")]
    NulPath,
    /// A file entry arrived without a body
    #[error("missing file body")]
    MissingBody,
    /// A non-file entry carried a body or nonzero size
    #[error("non-file entry has a body or nonzero size")]
    NonFilePayload,
    /// The constructor received metadata for another entry kind
    #[error("entry kind does not match constructor")]
    InvalidEntryKind,
    /// A supplied file body did not match its declared byte length
    #[error("file body length does not match its header")]
    BodyLengthMismatch,
    /// An earlier read or write failed and the archive cannot be resumed
    #[error("archive operation aborted after a prior error")]
    Aborted,
    /// An archive limit was exceeded
    #[error("limit exceeded: {0}")]
    LimitExceeded(&'static str),
    /// An archive entry carried a disallowed type
    #[error("disallowed entry type")]
    DisallowedEntryType,
    /// The archive was malformed
    #[error("malformed archive")]
    Malformed,
    /// A source, sink, or scratch operation failed
    #[error(transparent)]
    Io(#[from] io::Error),
}
