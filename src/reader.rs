//! Stateful, bounded archive reader

use crate::compress;
use crate::entry::{Meta, SafePath};
use crate::error::Error;
use crate::guard::HeaderGuard;
use crate::limits::SecurityLimits;
use crate::open::{Cap, Meter, OpenEntry, open_entry, read_error};
use futures::StreamExt;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};
use tokio_tar::{Archive, Entries};

type Source = Box<dyn AsyncRead + Unpin + Send>;

/// A view of one validated archive header
pub struct EntryHeader<'a> {
    /// Relative path, without filesystem access
    pub path: &'a SafePath,
    /// Unix metadata and entry kind
    pub meta: &'a Meta,
    /// Declared body length
    pub size_bytes: u64,
}

/// One entry body, borrowed from the reader until dropped
pub struct ReadEntry<'a> {
    inner: OpenEntry<Archive<Source>>,
    failed: Arc<AtomicBool>,
    reader: PhantomData<&'a mut Reader>,
}

impl ReadEntry<'_> {
    /// Return the validated header without cloning its path or metadata
    pub fn header(&self) -> EntryHeader<'_> {
        EntryHeader {
            path: &self.inner.path,
            meta: &self.inner.meta,
            size_bytes: self.inner.size_bytes,
        }
    }
}

impl AsyncRead for ReadEntry<'_> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_read(cx, buf);
        if matches!(result, Poll::Ready(Err(_))) {
            this.failed.store(true, Ordering::Relaxed);
        }
        result
    }
}

/// A sequential tar or tar.zst reader with bounded decoding
///
/// # Example
///
/// ```
/// use etar::{EntryPath, Format, Kind, Meta, ReadLimits, Reader, WriteEntry, Writer};
/// use std::io::Cursor;
/// use tokio::io::AsyncReadExt;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let mut writer = Writer::new(Vec::new(), Format::Tar);
/// let path = EntryPath::new("notes.txt")?;
/// let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
/// writer.append(WriteEntry::file(path, meta, 5, Cursor::new(b"hello".to_vec()))?).await?;
/// let archive = writer.finish().await?.sink;
///
/// let mut reader = Reader::open(Cursor::new(archive), ReadLimits::default()).await?;
/// let mut entry = reader.next_entry().await?.expect("one file");
/// let mut body = Vec::new();
/// entry.read_to_end(&mut body).await?;
/// assert_eq!(entry.header().path.as_bytes(), b"notes.txt");
/// assert_eq!(body, b"hello");
/// drop(entry);
/// assert!(reader.next_entry().await?.is_none());
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok(())
/// # }
/// ```
pub struct Reader {
    archive: Option<Archive<Source>>,
    entries: Option<Entries<Source>>,
    limits: SecurityLimits,
    encoded: Arc<AtomicU64>,
    decoded: Arc<AtomicU64>,
    body_failed: Arc<AtomicBool>,
    seen: u64,
    done: bool,
}

struct AbortOnDrop {
    failed: Arc<AtomicBool>,
    armed: bool,
}

impl AbortOnDrop {
    fn complete(mut self) {
        self.armed = false;
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.failed.store(true, Ordering::Relaxed);
        }
    }
}

impl Reader {
    /// Open an archive, detecting zstd from its magic bytes
    pub async fn open<R>(source: R, limits: SecurityLimits) -> Result<Self, Error>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let encoded = Arc::new(AtomicU64::new(0));
        let decoded = Arc::new(AtomicU64::new(0));
        let source = compress::decoded(Meter::new(source, encoded.clone())).await?;
        let source = Cap::new(
            source,
            decoded.clone(),
            encoded.clone(),
            limits.max_total_bytes,
            limits.max_compression_ratio,
        );
        let guarded = HeaderGuard::new(source, limits.max_metadata_bytes);
        let mut archive = Archive::new(Box::new(guarded) as Source);
        let entries = archive.entries().map_err(|_| Error::Malformed)?;
        Ok(Self {
            archive: Some(archive),
            entries: Some(entries),
            limits,
            encoded,
            decoded,
            body_failed: Arc::new(AtomicBool::new(false)),
            seen: 0,
            done: false,
        })
    }

    /// Return the next entry; a clean end verifies the tar trailer and decoder
    pub async fn next_entry(&mut self) -> Result<Option<ReadEntry<'_>>, Error> {
        if self.body_failed.load(Ordering::Relaxed) {
            return Err(Error::Aborted);
        }
        if self.done {
            return Ok(None);
        }
        let abort = AbortOnDrop {
            failed: self.body_failed.clone(),
            armed: true,
        };
        let next = match self.entries.as_mut() {
            Some(entries) => entries.next().await,
            None => return Err(Error::Malformed),
        };
        match next {
            Some(Ok(entry)) => {
                let accepted = self.accept(entry).map(Some);
                if accepted.is_ok() {
                    abort.complete();
                }
                accepted
            }
            Some(Err(error)) => Err(read_error(
                error,
                &self.decoded,
                self.limits.max_total_bytes,
            )),
            None => {
                self.verify_end().await?;
                self.done = true;
                abort.complete();
                Ok(None)
            }
        }
    }

    /// Number of encoded bytes pulled from the source
    pub fn encoded_bytes(&self) -> u64 {
        self.encoded.load(Ordering::Relaxed)
    }

    /// Number of decoded bytes pulled, including tar headers and padding
    pub fn decoded_bytes(&self) -> u64 {
        self.decoded.load(Ordering::Relaxed)
    }

    fn accept(&mut self, entry: tokio_tar::Entry<Archive<Source>>) -> Result<ReadEntry<'_>, Error> {
        self.seen += 1;
        if self.seen > self.limits.max_entries {
            return Err(Error::LimitExceeded("max_entries"));
        }
        let inner = open_entry(entry, &self.limits)?;
        Ok(ReadEntry {
            inner,
            failed: self.body_failed.clone(),
            reader: PhantomData,
        })
    }

    async fn verify_end(&mut self) -> Result<(), Error> {
        self.entries.take();
        let archive = self.archive.take().ok_or(Error::Malformed)?;
        let mut source = archive.into_inner().map_err(|_| Error::Malformed)?;
        let mut block = [0u8; 8192];
        let mut trailing = 0usize;
        loop {
            let count = source
                .read(&mut block)
                .await
                .map_err(|error| read_error(error, &self.decoded, self.limits.max_total_bytes))?;
            if count == 0 {
                break;
            }
            if block[..count].iter().any(|byte| *byte != 0) {
                return Err(Error::Malformed);
            }
            trailing = trailing.saturating_add(count);
        }
        if trailing < 512 {
            return Err(Error::Malformed);
        }
        Ok(())
    }
}

/// Counts from a complete archive validation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationReport {
    /// Number of user-visible entries
    pub entries: u64,
    /// Encoded source bytes consumed
    pub encoded_bytes: u64,
    /// Decoded bytes consumed, including tar structure
    pub decoded_bytes: u64,
}

/// Validate every header, body, trailer, and compressed frame
///
/// # Example
///
/// ```
/// use etar::{ReadLimits, validate};
/// use std::io::Cursor;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let empty_tar = vec![0u8; 1024];
/// let report = validate(Cursor::new(empty_tar), ReadLimits::default()).await?;
/// assert_eq!(report.entries, 0);
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok(())
/// # }
/// ```
pub async fn validate<R>(source: R, limits: SecurityLimits) -> Result<ValidationReport, Error>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut reader = Reader::open(source, limits).await?;
    let mut entries = 0;
    while let Some(mut entry) = reader.next_entry().await? {
        tokio::io::copy(&mut entry, &mut tokio::io::sink())
            .await
            .map_err(|error| read_error(error, &reader.decoded, reader.limits.max_total_bytes))?;
        entries += 1;
    }
    Ok(ValidationReport {
        entries,
        encoded_bytes: reader.encoded_bytes(),
        decoded_bytes: reader.decoded_bytes(),
    })
}
