//! Sequential tar and tar.zst writer

use crate::entry::{EntryPath, Kind, Meta};
use crate::error::Error;
use crate::pack;
use crate::pax;
use crate::settings::compression::LEVEL;
use async_compression::Level;
use async_compression::tokio::write::ZstdEncoder;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio_tar::Builder;

/// A checked entry supplied to a writer
///
/// # Example
///
/// ```
/// use etar::{EntryPath, Kind, Meta, WriteEntry};
/// use std::io::Cursor;
///
/// # fn main() -> Result<(), etar::Error> {
/// let path = EntryPath::new("notes.txt")?;
/// let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
/// let file = WriteEntry::file(path, meta, 5, Cursor::new(b"hello".to_vec()))?;
/// let _ = file;
/// # Ok(())
/// # }
/// ```
pub struct WriteEntry<B> {
    /// The relative path under which the entry is stored
    pub(crate) path: EntryPath,
    /// The entry's unix metadata
    pub(crate) meta: Meta,
    /// The body length, which the body must yield exactly
    pub(crate) size: u64,
    /// The body bytes, empty for every kind but a file
    pub(crate) body: B,
}

impl<B: AsyncRead + Unpin + Send> WriteEntry<B> {
    /// Create a regular file with an exact declared body length
    pub fn file(path: EntryPath, meta: Meta, size: u64, body: B) -> Result<Self, Error> {
        if !matches!(meta.kind, Kind::File) {
            return Err(Error::InvalidEntryKind);
        }
        pax::check(&meta)?;
        Ok(Self {
            path,
            meta,
            size,
            body,
        })
    }
}

impl WriteEntry<tokio::io::Empty> {
    /// Create a directory, link, device, or fifo without a body
    pub fn metadata(path: EntryPath, meta: Meta) -> Result<Self, Error> {
        if matches!(meta.kind, Kind::File) {
            return Err(Error::InvalidEntryKind);
        }
        pax::check(&meta)?;
        pack::check_device(&meta.kind)?;
        Ok(Self {
            path,
            meta,
            size: 0,
            body: tokio::io::empty(),
        })
    }
}

enum Inner<W: AsyncWrite + Unpin + Send + 'static> {
    Tar(Builder<Deferred<W>>),
    TarZstd(Builder<Deferred<ZstdEncoder<W>>>),
}

/// Sequential archive writer; call `finish` to finalize the tar and zstd trailers
///
/// # Example
///
/// ```
/// use etar::{EntryPath, Kind, Meta, WriteEntry, Writer};
/// use std::io::Cursor;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let mut writer = Writer::tar(Vec::new());
/// let path = EntryPath::new("notes.txt")?;
/// let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
/// let file = WriteEntry::file(path, meta, 5, Cursor::new(b"hello".to_vec()))?;
/// writer.append(file).await?;
/// let archive = writer.finish().await?;
/// assert_eq!(&archive[257..262], b"ustar");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok(())
/// # }
/// ```
pub struct Writer<W: AsyncWrite + Unpin + Send + 'static> {
    inner: Inner<W>,
    failed: bool,
}

impl<W> Writer<W>
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    /// Begin an uncompressed tar on the caller-owned sink
    pub fn tar(sink: W) -> Self {
        Self::from_inner(Inner::Tar(Builder::new(Deferred(sink))))
    }

    /// Begin a tar in one zstd frame, compressed at level 3 on the calling task
    pub fn tar_zst(sink: W) -> Self {
        let encoder = ZstdEncoder::with_quality(sink, Level::Precise(LEVEL));
        Self::from_inner(Inner::TarZstd(Builder::new(Deferred(encoder))))
    }

    fn from_inner(inner: Inner<W>) -> Self {
        Self {
            inner,
            failed: false,
        }
    }

    /// Append exactly one entry in archive order
    pub async fn append<B>(&mut self, entry: WriteEntry<B>) -> Result<(), Error>
    where
        B: AsyncRead + Unpin + Send,
    {
        if self.failed {
            return Err(Error::Aborted);
        }
        self.failed = true;
        let result = match &mut self.inner {
            Inner::Tar(builder) => pack::append(builder, entry).await,
            Inner::TarZstd(builder) => pack::append(builder, entry).await,
        };
        self.failed = result.is_err();
        result
    }

    /// Emit archive terminators, finish compression, shut the sink down, and return it
    pub async fn finish(self) -> Result<W, Error> {
        if self.failed {
            return Err(Error::Aborted);
        }
        match self.inner {
            Inner::Tar(builder) => {
                let mut sink = builder.into_inner().await?.0;
                sink.shutdown().await?;
                Ok(sink)
            }
            Inner::TarZstd(builder) => {
                let mut encoder = builder.into_inner().await?.0;
                encoder.shutdown().await?;
                Ok(encoder.into_inner())
            }
        }
    }
}

/// A sink that ignores the flush the tar builder issues after each entry body, so zstd keeps one
/// block stream across entries; `finish` shuts it down instead
struct Deferred<W>(W);

impl<W: AsyncWrite + Unpin> AsyncWrite for Deferred<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
    }
}
