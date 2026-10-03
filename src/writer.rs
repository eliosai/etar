//! Sequential tar and tar.zst writer

use crate::entry::{Kind, Meta, SafePath};
use crate::error::Error;
use crate::pack::{self, PackEntry};
use crate::pax;
use crate::zstd::Zstd;
use async_compression::tokio::write::ZstdEncoder;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio_tar::Builder;

/// Output encoding
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Uncompressed tar
    Tar,
    /// Tar wrapped in a single zstd frame
    TarZstd,
}

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
    inner: PackEntry<B>,
}

impl<B: AsyncRead + Unpin + Send> WriteEntry<B> {
    /// Create a regular file with an exact declared body length
    pub fn file(path: SafePath, meta: Meta, size: u64, body: B) -> Result<Self, Error> {
        if !matches!(meta.kind, Kind::File) {
            return Err(Error::InvalidEntryKind);
        }
        pax::check(&meta)?;
        Ok(Self {
            inner: PackEntry {
                path,
                meta,
                size,
                body: Some(body),
            },
        })
    }
}

impl WriteEntry<tokio::io::Empty> {
    /// Create a directory or link without a body
    pub fn metadata(path: SafePath, meta: Meta) -> Result<Self, Error> {
        if matches!(meta.kind, Kind::File) {
            return Err(Error::InvalidEntryKind);
        }
        pax::check(&meta)?;
        pack::check_device(&meta.kind)?;
        Ok(Self {
            inner: PackEntry {
                path,
                meta,
                size: 0,
                body: None,
            },
        })
    }
}

/// Finalized sink and number of encoded bytes written
pub struct WriteOutput<W> {
    /// The caller-owned sink after all trailers are written
    pub sink: W,
    /// Bytes successfully written to the sink
    pub encoded_bytes: u64,
}

enum Inner<W: AsyncWrite + Unpin + Send + 'static> {
    Tar(Builder<Deferred<Counted<W>>>),
    TarZstd(Builder<Deferred<ZstdEncoder<Counted<W>>>>),
}

/// Sequential archive writer; call `finish` to finalize the tar and zstd trailers
///
/// # Example
///
/// ```
/// use etar::{EntryPath, Format, Kind, Meta, WriteEntry, Writer};
/// use std::io::Cursor;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let mut writer = Writer::new(Vec::new(), Format::Tar);
/// let path = EntryPath::new("notes.txt")?;
/// let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
/// let file = WriteEntry::file(path, meta, 5, Cursor::new(b"hello".to_vec()))?;
/// writer.append(file).await?;
/// let archive = writer.finish().await?.sink;
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

struct AbortOnDrop<'a> {
    failed: &'a mut bool,
    armed: bool,
}

impl AbortOnDrop<'_> {
    fn complete(mut self) {
        self.armed = false;
    }
}

impl Drop for AbortOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed {
            *self.failed = true;
        }
    }
}

impl<W> Writer<W>
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    /// Begin a tar or tar.zst archive on the caller-owned sink
    pub fn new(sink: W, format: Format) -> Self {
        match format {
            Format::Tar => Self::from_inner(Inner::Tar(Builder::new(Deferred(Counted::new(sink))))),
            Format::TarZstd => Self::with_zstd(sink, Zstd::default()),
        }
    }

    /// Begin a tar.zst archive with a chosen zstd level and worker count
    pub fn with_zstd(sink: W, zstd: Zstd) -> Self {
        let encoder = zstd.encoder(Counted::new(sink));
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
        let abort = AbortOnDrop {
            failed: &mut self.failed,
            armed: true,
        };
        let result = match &mut self.inner {
            Inner::Tar(builder) => pack::append(builder, entry.inner).await,
            Inner::TarZstd(builder) => pack::append(builder, entry.inner).await,
        };
        if result.is_ok() {
            abort.complete();
        }
        result
    }

    /// Emit archive terminators, finish compression, and return the sink
    pub async fn finish(self) -> Result<WriteOutput<W>, Error> {
        if self.failed {
            return Err(Error::Aborted);
        }
        let sink = match self.inner {
            Inner::Tar(builder) => {
                let mut sink = builder.into_inner().await?.0;
                sink.shutdown().await?;
                sink
            }
            Inner::TarZstd(builder) => {
                let mut encoder = builder.into_inner().await?.0;
                encoder.shutdown().await?;
                encoder.into_inner()
            }
        };
        Ok(WriteOutput {
            encoded_bytes: sink.bytes,
            sink: sink.inner,
        })
    }
}

struct Counted<W> {
    inner: W,
    bytes: u64,
}

impl<W> Counted<W> {
    fn new(inner: W) -> Self {
        Self { inner, bytes: 0 }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for Counted<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_write(cx, buf) {
            Poll::Ready(Ok(bytes)) => {
                this.bytes = this.bytes.saturating_add(bytes as u64);
                Poll::Ready(Ok(bytes))
            }
            other => other,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
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
