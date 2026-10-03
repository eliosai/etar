//! zstd compression options for tar.zst output

use crate::error::Error;
use crate::settings::zstd::{DEFAULT_LEVEL, MAX_LEVEL, MAX_WORKERS, MIN_LEVEL};
use async_compression::Level;
use async_compression::tokio::write::ZstdEncoder;
use async_compression::zstd::CParameter;
use tokio::io::AsyncWrite;

/// Checked zstd level and compression worker count for tar.zst output
///
/// Level `0` selects zstd's default. Workers `0` compresses on the calling
/// task; `1..=256` hands compression to that many zstd worker threads.
///
/// # Example
///
/// ```
/// use etar::{Error, Zstd};
///
/// let fast = Zstd::new(1, 4).unwrap();
/// assert!(matches!(Zstd::new(23, 0), Err(Error::InvalidZstdLevel(23))));
/// # let _ = fast;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Zstd {
    level: i32,
    workers: u32,
}

impl Zstd {
    /// Check a level in `-131072..=22` and a worker count in `0..=256`
    pub fn new(level: i32, workers: u32) -> Result<Self, Error> {
        if !(MIN_LEVEL..=MAX_LEVEL).contains(&level) {
            return Err(Error::InvalidZstdLevel(level));
        }
        if workers > MAX_WORKERS {
            return Err(Error::InvalidZstdWorkers(workers));
        }
        Ok(Self { level, workers })
    }

    /// Build the encoder over `sink` with these options
    pub(crate) fn encoder<W: AsyncWrite>(self, sink: W) -> ZstdEncoder<W> {
        let level = Level::Precise(self.level);
        match self.workers {
            0 => ZstdEncoder::with_quality(sink, level),
            workers => ZstdEncoder::with_quality_and_params(
                sink,
                level,
                &[CParameter::nb_workers(workers)],
            ),
        }
    }
}

impl Default for Zstd {
    /// Level 3 on the calling task, the encoder's behavior before options existed
    fn default() -> Self {
        Self {
            level: DEFAULT_LEVEL,
            workers: 0,
        }
    }
}
