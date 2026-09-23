//! Disk-backed spool that hashes a body while buffering it in RAM or on NVMe

use blake3::Hash;
use bytes::{Bytes, BytesMut};
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use tempfile::NamedTempFile;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufWriter, ReadBuf};

/// Read granularity pulled from the body each step
const FRAME_BYTES: usize = 256 * 1024;
/// Buffered-writer capacity coalescing spill writes
const SPILL_BUF_BYTES: usize = 1024 * 1024;
/// Default in-memory threshold before spilling to disk
const DEFAULT_INLINE_MAX: u64 = 1024 * 1024;
/// Ceiling on eager inline pre-allocation, bounding a large configured threshold
const MAX_EAGER_ALLOC: u64 = 4 * 1024 * 1024;

/// Why spooling a body failed
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SpoolError {
    /// The underlying read or disk write failed
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Where spooling buffers and hands RAM off to disk
#[derive(Debug, Clone)]
pub struct SpoolOptions {
    inline_max: u64,
    scratch_dir: PathBuf,
}

impl SpoolOptions {
    /// Spool bodies past the threshold into temp files under this directory
    pub fn new(scratch_dir: impl Into<PathBuf>) -> Self {
        Self {
            inline_max: DEFAULT_INLINE_MAX,
            scratch_dir: scratch_dir.into(),
        }
    }

    /// Set the in-memory threshold before spilling to disk
    pub fn with_inline_max(mut self, bytes: u64) -> Self {
        self.inline_max = bytes;
        self
    }
}

/// A spooled body, in RAM or on NVMe, with its content hash
pub struct Spooled {
    hash: Hash,
    size: u64,
    body: Body,
}

/// The backing store of a spooled body
enum Body {
    Memory(Bytes),
    Disk(NamedTempFile),
}

impl Spooled {
    /// The BLAKE3 hash of the spooled bytes
    pub fn hash(&self) -> Hash {
        self.hash
    }

    /// The spooled byte length
    pub fn size_bytes(&self) -> u64 {
        self.size
    }

    /// Open a reader over the spooled bytes from the start
    pub fn into_reader(self) -> Result<SpooledReader, SpoolError> {
        match self.body {
            Body::Memory(bytes) => Ok(SpooledReader(Source::Memory(Cursor::new(bytes)))),
            Body::Disk(file) => {
                let reopened = tokio::fs::File::from_std(file.reopen()?);
                Ok(SpooledReader(Source::Disk(reopened)))
            }
        }
    }

    /// Open a reader over the spooled bytes without consuming the spool, so the
    /// same body can be read more than once, for an upload and a separate scan
    ///
    /// # Errors
    /// [`SpoolError`] if a disk-backed spool cannot be reopened
    pub fn reader(&self) -> Result<SpooledReader, SpoolError> {
        match &self.body {
            Body::Memory(bytes) => Ok(SpooledReader(Source::Memory(Cursor::new(bytes.clone())))),
            Body::Disk(file) => {
                let reopened = tokio::fs::File::from_std(file.reopen()?);
                Ok(SpooledReader(Source::Disk(reopened)))
            }
        }
    }

    /// Move the spooled bytes to `dir`/`name`, returning the path
    pub async fn place_in(self, dir: impl AsRef<Path>, name: &str) -> Result<PathBuf, SpoolError> {
        let path = dir.as_ref().join(single_component(name)?);
        match self.body {
            Body::Memory(bytes) => tokio::fs::write(&path, &bytes).await?,
            Body::Disk(file) => persist_or_copy(file, &path).await?,
        }
        Ok(path)
    }
}

/// Require a name of exactly one ordinary path component, rejecting traversal
fn single_component(name: &str) -> Result<&str, SpoolError> {
    let mut parts = Path::new(name).components();
    match (parts.next(), parts.next()) {
        (Some(Component::Normal(only)), None)
            if only.len() == name.len() && !name.as_bytes().contains(&0) =>
        {
            Ok(name)
        }
        _ => Err(invalid("name must be one path component")),
    }
}

/// Rename a temp file into place, copying it if the rename crosses a filesystem
async fn persist_or_copy(file: NamedTempFile, path: &Path) -> Result<(), SpoolError> {
    match file.persist(path) {
        Ok(_) => Ok(()),
        Err(persist) if persist.error.kind() == std::io::ErrorKind::CrossesDevices => {
            tokio::fs::copy(persist.file.path(), path).await?;
            Ok(())
        }
        Err(persist) => Err(SpoolError::Io(persist.error)),
    }
}

/// An invalid-input spool error carrying a static reason
fn invalid(reason: &'static str) -> SpoolError {
    SpoolError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        reason,
    ))
}

/// A reader over spooled bytes, from RAM or a spilled file
pub struct SpooledReader(Source);

/// The concrete source behind a spooled reader
enum Source {
    Memory(Cursor<Bytes>),
    Disk(tokio::fs::File),
}

impl AsyncRead for SpooledReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match &mut self.get_mut().0 {
            Source::Memory(cursor) => Pin::new(cursor).poll_read(cx, buf),
            Source::Disk(file) => Pin::new(file).poll_read(cx, buf),
        }
    }
}

/// Consume a body, hashing it while spooling to RAM then NVMe past the threshold
pub async fn capture<R>(mut body: R, opts: &SpoolOptions) -> Result<Spooled, SpoolError>
where
    R: AsyncRead + Unpin + Send,
{
    let mut accum = Accum::new(opts);
    let mut scratch = BytesMut::with_capacity(FRAME_BYTES);
    while body.read_buf(&mut scratch).await? != 0 {
        accum.push(&scratch).await?;
        scratch.clear();
    }
    accum.finish().await
}

/// Accumulates a body into RAM, spilling to a temp file past the threshold
struct Accum<'a> {
    hasher: blake3::Hasher,
    inline: BytesMut,
    spill: Option<(BufWriter<tokio::fs::File>, NamedTempFile)>,
    total: u64,
    opts: &'a SpoolOptions,
}

impl<'a> Accum<'a> {
    fn new(opts: &'a SpoolOptions) -> Self {
        let capacity = opts.inline_max.min(MAX_EAGER_ALLOC) as usize;
        Self {
            hasher: blake3::Hasher::new(),
            inline: BytesMut::with_capacity(capacity),
            spill: None,
            total: 0,
            opts,
        }
    }

    async fn push(&mut self, chunk: &[u8]) -> Result<(), SpoolError> {
        self.hasher.update(chunk);
        if let Some((writer, _)) = &mut self.spill {
            writer.write_all(chunk).await?;
        } else if self.total + chunk.len() as u64 > self.opts.inline_max {
            self.spill_over(chunk).await?;
        } else {
            self.inline.extend_from_slice(chunk);
        }
        self.total += chunk.len() as u64;
        Ok(())
    }

    async fn spill_over(&mut self, chunk: &[u8]) -> Result<(), SpoolError> {
        let temp = NamedTempFile::new_in(&self.opts.scratch_dir)?;
        let mut writer =
            BufWriter::with_capacity(SPILL_BUF_BYTES, tokio::fs::File::from_std(temp.reopen()?));
        writer.write_all(&self.inline).await?;
        writer.write_all(chunk).await?;
        self.inline = BytesMut::new();
        self.spill = Some((writer, temp));
        Ok(())
    }

    async fn finish(self) -> Result<Spooled, SpoolError> {
        let hash = self.hasher.finalize();
        if let Some((mut writer, temp)) = self.spill {
            writer.flush().await?;
            return Ok(Spooled {
                hash,
                size: self.total,
                body: Body::Disk(temp),
            });
        }
        Ok(Spooled {
            hash,
            size: self.total,
            body: Body::Memory(self.inline.freeze()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::io::Cursor as IoCursor;

    fn opts(dir: &Path, inline_max: u64) -> SpoolOptions {
        SpoolOptions::new(dir).with_inline_max(inline_max)
    }

    async fn read_back(spooled: Spooled) -> Vec<u8> {
        let mut reader = spooled.into_reader().unwrap();
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn test_small_body_stays_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let body = vec![7u8; 100];
        let spooled = capture(IoCursor::new(body.clone()), &opts(dir.path(), 1024))
            .await
            .unwrap();
        assert_eq!(spooled.size_bytes(), 100);
        assert_eq!(spooled.hash(), blake3::hash(&body));
        assert!(matches!(spooled.body, Body::Memory(_)));
        assert_eq!(read_back(spooled).await, body);
    }

    #[tokio::test]
    async fn test_large_body_spills_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let body = vec![3u8; 4096];
        let spooled = capture(IoCursor::new(body.clone()), &opts(dir.path(), 64))
            .await
            .unwrap();
        assert_eq!(spooled.size_bytes(), 4096);
        assert_eq!(spooled.hash(), blake3::hash(&body));
        assert!(matches!(spooled.body, Body::Disk(_)));
        assert_eq!(read_back(spooled).await, body);
    }

    #[tokio::test]
    async fn test_threshold_boundary_stays_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let body = vec![1u8; 64];
        let spooled = capture(IoCursor::new(body), &opts(dir.path(), 64))
            .await
            .unwrap();
        assert!(matches!(spooled.body, Body::Memory(_)));
    }

    #[tokio::test]
    async fn test_one_over_threshold_spills() {
        let dir = tempfile::tempdir().unwrap();
        let body = vec![1u8; 65];
        let spooled = capture(IoCursor::new(body), &opts(dir.path(), 64))
            .await
            .unwrap();
        assert!(matches!(spooled.body, Body::Disk(_)));
    }

    #[tokio::test]
    async fn test_empty_body_hashes_empty() {
        let dir = tempfile::tempdir().unwrap();
        let spooled = capture(IoCursor::new(Vec::new()), &opts(dir.path(), 64))
            .await
            .unwrap();
        assert_eq!(spooled.size_bytes(), 0);
        assert_eq!(spooled.hash(), blake3::hash(b""));
    }

    #[tokio::test]
    async fn test_place_in_writes_memory_body() {
        let dir = tempfile::tempdir().unwrap();
        let spooled = capture(IoCursor::new(b"abc".to_vec()), &opts(dir.path(), 1024))
            .await
            .unwrap();
        let path = spooled.place_in(dir.path(), "abc").await.unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"abc");
    }

    #[tokio::test]
    async fn test_place_in_persists_disk_body() {
        let dir = tempfile::tempdir().unwrap();
        let body = vec![9u8; 4096];
        let spooled = capture(IoCursor::new(body.clone()), &opts(dir.path(), 64))
            .await
            .unwrap();
        let path = spooled.place_in(dir.path(), "blob").await.unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap(), body);
    }

    #[tokio::test]
    async fn test_mid_stream_spill_preserves_order_and_hash() {
        let dir = tempfile::tempdir().unwrap();
        let body: Vec<u8> = (0..700 * 1024).map(|i| (i % 251) as u8).collect();
        let spooled = capture(IoCursor::new(body.clone()), &opts(dir.path(), 300 * 1024))
            .await
            .unwrap();
        assert!(matches!(spooled.body, Body::Disk(_)));
        assert_eq!(spooled.hash(), blake3::hash(&body));
        assert_eq!(read_back(spooled).await, body);
    }

    #[tokio::test]
    async fn test_partial_reads_hash_correctly() {
        let dir = tempfile::tempdir().unwrap();
        let body: Vec<u8> = (0..600 * 1024).map(|i| (i % 199) as u8).collect();
        let trickle = Trickle {
            data: body.clone(),
            pos: 0,
            step: 13,
        };
        let spooled = capture(trickle, &opts(dir.path(), 100 * 1024))
            .await
            .unwrap();
        assert_eq!(spooled.hash(), blake3::hash(&body));
        assert_eq!(read_back(spooled).await, body);
    }

    #[tokio::test]
    async fn test_place_in_rejects_traversal_names() {
        let dir = tempfile::tempdir().unwrap();
        for bad in [
            "../escape",
            "/etc/passwd",
            "a/b",
            "",
            ".",
            "a/",
            "a/.",
            "a//b",
            "a\0b",
        ] {
            let spooled = capture(IoCursor::new(b"x".to_vec()), &opts(dir.path(), 1024))
                .await
                .unwrap();
            assert!(
                spooled.place_in(dir.path(), bad).await.is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn test_place_in_across_filesystems() {
        let scratch = Path::new("/dev/shm");
        if !scratch.is_dir() {
            return;
        }
        let target = tempfile::tempdir().unwrap();
        let body = vec![5u8; 4096];
        let options = SpoolOptions::new(scratch).with_inline_max(64);
        let spooled = capture(IoCursor::new(body.clone()), &options)
            .await
            .unwrap();
        let path = spooled.place_in(target.path(), "blob").await.unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap(), body);
    }

    struct Trickle {
        data: Vec<u8>,
        pos: usize,
        step: usize,
    }

    impl AsyncRead for Trickle {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let me = self.get_mut();
            let want = me.step.min(buf.remaining()).min(me.data.len() - me.pos);
            buf.put_slice(&me.data[me.pos..me.pos + want]);
            me.pos += want;
            Poll::Ready(Ok(()))
        }
    }
}
