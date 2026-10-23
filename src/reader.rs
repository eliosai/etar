//! Stateful, bounded archive reader

use crate::compress;
use crate::entry::{EntryPath, Meta};
use crate::error::Error;
use crate::guard::HeaderGuard;
use crate::limits::ReadLimits;
use crate::open::{Cap, Tagged, open_entry, read_error};
use crate::settings::open::MAX_METADATA_BYTES;
use futures::StreamExt;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};
use tokio_tar::{Archive, Entries, Entry};

type Source = Box<dyn AsyncRead + Unpin + Send>;

/// A view of one validated archive header
pub struct EntryHeader<'a> {
    /// Relative path, without filesystem access
    pub path: &'a EntryPath,
    /// Unix metadata and entry kind
    pub meta: &'a Meta,
    /// Declared body length
    pub size_bytes: u64,
}

/// One entry body, borrowed from the reader until dropped
pub struct ReadEntry<'a> {
    path: EntryPath,
    meta: Meta,
    size_bytes: u64,
    body: Entry<Archive<Source>>,
    failed: Arc<AtomicBool>,
    reader: PhantomData<&'a mut Reader>,
}

impl ReadEntry<'_> {
    /// Return the validated header without cloning its path or metadata
    pub fn header(&self) -> EntryHeader<'_> {
        EntryHeader {
            path: &self.path,
            meta: &self.meta,
            size_bytes: self.size_bytes,
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
        let result = Pin::new(&mut this.body).poll_read(cx, buf);
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
/// use etar::{EntryPath, Kind, Meta, ReadLimits, Reader, WriteEntry, Writer};
/// use std::io::Cursor;
/// use tokio::io::AsyncReadExt;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let mut writer = Writer::tar(Vec::new());
/// let path = EntryPath::new("notes.txt")?;
/// let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
/// writer.append(WriteEntry::file(path, meta, 5, Cursor::new(b"hello".to_vec()))?).await?;
/// let archive = writer.finish().await?;
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
    limits: ReadLimits,
    decoded: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
    seen: u64,
}

impl Reader {
    /// Open an archive, detecting zstd from its magic bytes
    pub async fn open<R>(source: R, limits: ReadLimits) -> Result<Self, Error>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let decoded = Arc::new(AtomicU64::new(0));
        let source = compress::decoded(Tagged::new(source)).await?;
        let source = Cap::new(source, decoded.clone(), limits.max_total_bytes);
        let guarded = HeaderGuard::new(source, MAX_METADATA_BYTES);
        let mut archive = Archive::new(Box::new(guarded) as Source);
        let entries = archive.entries().map_err(|_| Error::Malformed)?;
        Ok(Self {
            archive: Some(archive),
            entries: Some(entries),
            limits,
            decoded,
            failed: Arc::new(AtomicBool::new(false)),
            seen: 0,
        })
    }

    /// Return the next entry; a clean end verifies the tar trailer and decoder
    pub async fn next_entry(&mut self) -> Result<Option<ReadEntry<'_>>, Error> {
        if self.failed.load(Ordering::Relaxed) {
            return Err(Error::Aborted);
        }
        let failed = self.failed.clone();
        failed.store(true, Ordering::Relaxed);
        let next = self.advance().await;
        failed.store(next.is_err(), Ordering::Relaxed);
        next
    }

    async fn advance(&mut self) -> Result<Option<ReadEntry<'_>>, Error> {
        let Some(entries) = self.entries.as_mut() else {
            return Ok(None);
        };
        match entries.next().await {
            Some(Ok(entry)) => self.accept(entry).await.map(Some),
            Some(Err(error)) => Err(read_error(
                error,
                &self.decoded,
                self.limits.max_total_bytes,
            )),
            None => self.verify_end().await.map(|()| None),
        }
    }

    async fn accept(&mut self, mut body: Entry<Archive<Source>>) -> Result<ReadEntry<'_>, Error> {
        self.seen += 1;
        if self.seen > self.limits.max_entries {
            return Err(Error::LimitExceeded("max_entries"));
        }
        let (path, meta, size_bytes) = open_entry(&mut body, &self.limits).await?;
        Ok(ReadEntry {
            path,
            meta,
            size_bytes,
            body,
            failed: self.failed.clone(),
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
