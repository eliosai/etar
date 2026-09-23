//! Streaming open of an untrusted tar or tar.zst into validated entries

use crate::entry::{Kind, Meta, SafePath};
use crate::error::Error;
use crate::limits::SecurityLimits;
use crate::settings::open::RATIO_FLOOR_BYTES;
use pin_project_lite::pin_project;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};
use tokio_tar::{Entry, EntryType};

/// One validated entry whose body is bounded as it is read
pub struct OpenEntry<R: AsyncRead + Unpin> {
    /// The validated relative path
    pub path: SafePath,
    /// The entry's unix metadata
    pub meta: Meta,
    /// The header-claimed body length, advisory only
    pub size_bytes: u64,
    body: Bounded<Entry<R>>,
}

impl<R: AsyncRead + Unpin> AsyncRead for OpenEntry<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().body).poll_read(cx, buf)
    }
}

/// Validate one entry and wrap its body with the byte bounds
pub fn open_entry<R>(entry: Entry<R>, limits: &SecurityLimits) -> Result<OpenEntry<R>, Error>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let path = validate_path(&entry, limits)?;
    let kind = classify(&entry, limits)?;
    let size_bytes = entry.header().size().map_err(|_| Error::Malformed)?;
    if !matches!(&kind, Kind::File) && size_bytes != 0 {
        return Err(Error::NonFilePayload);
    }
    if size_bytes > limits.max_entry_bytes {
        return Err(Error::LimitExceeded("max_entry_bytes"));
    }
    let meta = read_meta(&entry, kind)?;
    let body = Bounded::new(entry, limits.max_entry_bytes);
    Ok(OpenEntry {
        path,
        meta,
        size_bytes,
        body,
    })
}

/// Validate the entry path against length and traversal rules
fn validate_path<R>(entry: &Entry<R>, limits: &SecurityLimits) -> Result<SafePath, Error>
where
    R: AsyncRead + Unpin,
{
    let raw = entry.path().map_err(|_| Error::Malformed)?;
    if raw.as_os_str().len() > limits.max_path_len {
        return Err(Error::LimitExceeded("max_path_len"));
    }
    SafePath::new(&raw)
}

/// Map the tar entry type to a kind, rejecting dangerous types
fn classify<R>(entry: &Entry<R>, limits: &SecurityLimits) -> Result<Kind, Error>
where
    R: AsyncRead + Unpin,
{
    match entry.header().entry_type() {
        EntryType::Regular | EntryType::Continuous => Ok(Kind::File),
        EntryType::Directory => Ok(Kind::Dir),
        EntryType::Symlink if limits.allow_symlinks => {
            link(entry).map(|target| Kind::Symlink { target })
        }
        EntryType::Link if limits.allow_hardlinks => {
            link(entry).map(|target| Kind::Hardlink { target })
        }
        _ => Err(Error::DisallowedEntryType),
    }
}

/// Read a link entry's untrusted target
fn link<R>(entry: &Entry<R>) -> Result<PathBuf, Error>
where
    R: AsyncRead + Unpin,
{
    let name = entry.link_name().map_err(|_| Error::Malformed)?;
    Ok(name.ok_or(Error::Malformed)?.into_owned())
}

/// Read the entry's unix metadata from its header
fn read_meta<R>(entry: &Entry<R>, kind: Kind) -> Result<Meta, Error>
where
    R: AsyncRead + Unpin,
{
    let header = entry.header();
    Ok(Meta {
        mode: header.mode().map_err(|_| Error::Malformed)?,
        uid: small_id(header.uid().map_err(|_| Error::Malformed)?)?,
        gid: small_id(header.gid().map_err(|_| Error::Malformed)?)?,
        mtime_unix_secs: header.mtime().map_err(|_| Error::Malformed)?,
        kind,
    })
}

/// Narrow a tar uid or gid to the kernel's u32 width, rejecting overflow
fn small_id(id: u64) -> Result<u32, Error> {
    u32::try_from(id).map_err(|_| Error::Malformed)
}

pin_project! {
    /// Counts source bytes pulled, including buffered read-ahead, for the ratio
    pub struct Meter<R> {
        #[pin]
        inner: R,
        count: Arc<AtomicU64>,
    }
}

impl<R> Meter<R> {
    pub fn new(inner: R, count: Arc<AtomicU64>) -> Self {
        Self { inner, count }
    }
}

impl<R: AsyncRead> AsyncRead for Meter<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let this = self.project();
        let result = this.inner.poll_read(cx, buf);
        let read = (buf.filled().len() - before) as u64;
        if read > 0 {
            this.count.fetch_add(read, Ordering::Relaxed);
        }
        match result {
            Poll::Ready(Err(error)) => {
                Poll::Ready(Err(io::Error::new(error.kind(), Error::Io(error))))
            }
            other => other,
        }
    }
}

pin_project! {
    /// Caps one entry's body and fails reads once the stream advances past it
    struct Bounded<R> {
        #[pin]
        inner: R,
        remaining: u64,
        failed: bool,
    }
}

impl<R> Bounded<R> {
    fn new(inner: R, entry_max: u64) -> Self {
        Self {
            inner,
            remaining: entry_max,
            failed: false,
        }
    }
}

impl<R: AsyncRead> AsyncRead for Bounded<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.project();
        if *this.failed {
            return Poll::Ready(Err(cap_error("aborted entry body")));
        }
        let before = buf.filled().len();
        let result = this.inner.poll_read(cx, buf);
        let read = (buf.filled().len() - before) as u64;
        if read > *this.remaining {
            buf.set_filled(before);
            *this.failed = true;
            return Poll::Ready(Err(cap_error("max_entry_bytes")));
        }
        *this.remaining -= read;
        result
    }
}

pin_project! {
    /// Bounds total decoded bytes and the decompression ratio mid-decode
    pub struct Cap<R> {
        #[pin]
        inner: R,
        total: Arc<AtomicU64>,
        compressed: Arc<AtomicU64>,
        max: u64,
        max_ratio: u64,
        failed: bool,
    }
}

impl<R> Cap<R> {
    pub fn new(
        inner: R,
        total: Arc<AtomicU64>,
        compressed: Arc<AtomicU64>,
        max: u64,
        max_ratio: u64,
    ) -> Self {
        Self {
            inner,
            total,
            compressed,
            max,
            max_ratio,
            failed: false,
        }
    }
}

impl<R: AsyncRead> AsyncRead for Cap<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.project();
        if *this.failed {
            return Poll::Ready(Err(cap_error("aborted")));
        }
        let before = buf.filled().len();
        let result = this.inner.poll_read(cx, buf);
        let read = (buf.filled().len() - before) as u64;
        if read > 0
            && let Some(reason) = breach(
                this.total,
                this.compressed,
                *this.max,
                *this.max_ratio,
                read,
            )
        {
            buf.set_filled(before);
            *this.failed = true;
            return Poll::Ready(Err(cap_error(reason)));
        }
        result
    }
}

/// Charge decoded bytes and report which cap, if any, is now breached
fn breach(
    total: &AtomicU64,
    compressed: &AtomicU64,
    max: u64,
    max_ratio: u64,
    read: u64,
) -> Option<&'static str> {
    let now = total.fetch_add(read, Ordering::Relaxed) + read;
    if now > max {
        return Some("max_total_bytes");
    }
    let input = compressed.load(Ordering::Relaxed);
    if now > RATIO_FLOOR_BYTES && input > 0 && now > input.saturating_mul(max_ratio) {
        return Some("max_compression_ratio");
    }
    None
}

/// Classify a parser read failure as a total-cap breach or malformed input
pub fn read_error(error: std::io::Error, total: &AtomicU64, max: u64) -> Error {
    if total.load(Ordering::Relaxed) > max {
        return Error::LimitExceeded("max_total_bytes");
    }
    match error
        .get_ref()
        .and_then(|source| source.downcast_ref::<Error>())
    {
        Some(Error::LimitExceeded(which)) => Error::LimitExceeded(which),
        Some(Error::DisallowedEntryType) => Error::DisallowedEntryType,
        Some(Error::Io(_)) => Error::Io(error),
        _ => Error::Malformed,
    }
}

/// A read error standing for a breached byte cap
fn cap_error(which: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, Error::LimitExceeded(which))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::Reader;
    use pretty_assertions::assert_eq;
    use std::io::Cursor;
    use tokio::io::AsyncReadExt;
    use tokio_tar::{Builder, Header};

    async fn tar_of(entries: &[(&str, EntryType, &[u8])]) -> Vec<u8> {
        let mut builder = Builder::new(Vec::new());
        for (path, kind, data) in entries {
            let mut header = Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_entry_type(*kind);
            builder
                .append_data(&mut header, path, &data[..])
                .await
                .unwrap();
        }
        builder.into_inner().await.unwrap()
    }

    async fn collect(
        tar: Vec<u8>,
        limits: SecurityLimits,
    ) -> Vec<Result<(SafePath, Vec<u8>), Error>> {
        let mut reader = Reader::open(Cursor::new(tar), limits).await.unwrap();
        let mut out = Vec::new();
        loop {
            let item = match reader.next_entry().await {
                Ok(Some(entry)) => Ok(entry),
                Ok(None) => break,
                Err(error) => Err(error),
            };
            match item {
                Ok(mut entry) => {
                    let path = entry.header().path.clone();
                    let mut body = Vec::new();
                    match entry.read_to_end(&mut body).await {
                        Ok(_) => out.push(Ok((path, body))),
                        Err(error) => out.push(Err(Error::Io(error))),
                    }
                }
                Err(error) => {
                    out.push(Err(error));
                    break;
                }
            }
        }
        out
    }

    #[tokio::test]
    async fn test_round_trips_a_file() {
        let tar = tar_of(&[("bin/sh", EntryType::Regular, b"hello")]).await;
        let got = collect(tar, SecurityLimits::default()).await;
        assert_eq!(got.len(), 1);
        let (path, body) = got[0].as_ref().unwrap();
        assert_eq!(path.as_path(), std::path::Path::new("bin/sh"));
        assert_eq!(body, b"hello");
    }

    #[tokio::test]
    async fn test_max_entries_is_enforced() {
        let tar = tar_of(&[
            ("a", EntryType::Regular, b"x"),
            ("b", EntryType::Regular, b"y"),
        ])
        .await;
        let got = collect(tar, SecurityLimits::default().with_max_entries(1)).await;
        assert!(matches!(
            got.last(),
            Some(Err(Error::LimitExceeded("max_entries")))
        ));
    }

    #[tokio::test]
    async fn test_max_entry_bytes_is_enforced() {
        let tar = tar_of(&[("big", EntryType::Regular, b"0123456789")]).await;
        let got = collect(tar, SecurityLimits::default().with_max_entry_bytes(4)).await;
        assert!(matches!(
            got[0],
            Err(Error::LimitExceeded("max_entry_bytes"))
        ));
    }

    #[tokio::test]
    async fn test_device_entry_is_rejected() {
        let tar = tar_of(&[("dev/null", EntryType::Char, b"")]).await;
        let got = collect(tar, SecurityLimits::default()).await;
        assert!(matches!(got[0], Err(Error::DisallowedEntryType)));
    }

    #[tokio::test]
    async fn test_symlinks_can_be_disallowed() {
        let mut builder = Builder::new(Vec::new());
        let mut header = Header::new_gnu();
        header.set_size(0);
        header.set_entry_type(EntryType::Symlink);
        header.set_link_name("/etc/passwd").unwrap();
        header.set_path("link").unwrap();
        header.set_cksum();
        builder.append(&header, &b""[..]).await.unwrap();
        let tar = builder.into_inner().await.unwrap();
        let got = collect(tar, SecurityLimits::default().with_symlinks(false)).await;
        assert!(matches!(got[0], Err(Error::DisallowedEntryType)));
    }

    #[tokio::test]
    async fn test_hardlinks_can_be_disallowed() {
        let mut builder = Builder::new(Vec::new());
        let mut header = Header::new_gnu();
        header.set_size(0);
        header.set_entry_type(EntryType::Link);
        header.set_link_name("target").unwrap();
        header.set_path("hard").unwrap();
        header.set_cksum();
        builder.append(&header, &b""[..]).await.unwrap();
        let tar = builder.into_inner().await.unwrap();
        let got = collect(tar, SecurityLimits::default().with_hardlinks(false)).await;
        assert!(matches!(got[0], Err(Error::DisallowedEntryType)));
    }

    #[tokio::test]
    async fn test_traversal_entry_is_rejected() {
        let tar = raw_path_entry("../../etc/passwd", b"x");
        let got = collect(tar, SecurityLimits::default()).await;
        assert!(
            matches!(got[0], Err(Error::ParentEscape)),
            "{:?}",
            got.first()
        );
    }

    #[tokio::test]
    async fn test_total_decoded_cap_aborts_oversized_body() {
        let tar = tar_of(&[("f", EntryType::Regular, &[b'x'; 4096])]).await;
        let got = collect(tar, SecurityLimits::default().with_max_total_bytes(1024)).await;
        assert!(matches!(got[0], Err(Error::Io(_))));
    }

    #[tokio::test]
    async fn test_long_name_header_bomb_is_capped() {
        let tar = long_name_bomb(64 * 1024, 64 * 1024);
        let got = collect(
            tar,
            SecurityLimits::default().with_max_total_bytes(8 * 1024),
        )
        .await;
        assert!(
            matches!(
                got.first(),
                Some(Err(Error::LimitExceeded("max_total_bytes")))
            ),
            "{:?}",
            got.first()
        );
    }

    #[tokio::test]
    async fn test_compression_ratio_bomb_is_cut_mid_decode() {
        use async_compression::tokio::write::ZstdEncoder;
        use tokio::io::AsyncWriteExt;
        let tar = tar_of(&[("zeros", EntryType::Regular, &vec![0u8; 16 * 1024 * 1024])]).await;
        let mut encoder = ZstdEncoder::new(Vec::new());
        encoder.write_all(&tar).await.unwrap();
        encoder.shutdown().await.unwrap();
        let limits = SecurityLimits::default().with_max_compression_ratio(50);
        let got = collect(encoder.into_inner(), limits).await;
        match &got[0] {
            Err(Error::Io(error)) => {
                assert_eq!(error.to_string(), "limit exceeded: max_compression_ratio")
            }
            other => panic!("expected ratio cap, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_ratio_limit_is_reported_while_parsing_an_extension() {
        use async_compression::tokio::write::ZstdEncoder;
        use tokio::io::AsyncWriteExt;

        let tar = long_name_bomb(16 * 1024 * 1024, 16 * 1024 * 1024);
        let mut encoder = ZstdEncoder::new(Vec::new());
        encoder.write_all(&tar).await.unwrap();
        encoder.shutdown().await.unwrap();

        let limits = SecurityLimits::default()
            .with_max_compression_ratio(50)
            .with_max_metadata_bytes(32 * 1024 * 1024);
        let got = collect(encoder.into_inner(), limits).await;
        assert!(matches!(
            got.first(),
            Some(Err(Error::LimitExceeded("max_compression_ratio")))
        ));
    }

    fn raw_path_entry(name: &str, body: &[u8]) -> Vec<u8> {
        let mut base = Header::new_gnu();
        base.set_entry_type(EntryType::Regular);
        base.set_size(body.len() as u64);
        base.set_mode(0o644);
        base.set_uid(0);
        base.set_gid(0);
        base.set_mtime(0);
        let mut header = *base.as_bytes();
        header[..100].fill(0);
        header[..name.len()].copy_from_slice(name.as_bytes());
        write_cksum(&mut header);
        let mut tar = header.to_vec();
        tar.extend_from_slice(body);
        let pad = (512 - tar.len() % 512) % 512;
        tar.extend(std::iter::repeat_n(0u8, pad + 1024));
        tar
    }

    fn write_cksum(header: &mut [u8; 512]) {
        header[148..156].fill(b' ');
        let sum: u32 = header.iter().map(|&byte| byte as u32).sum();
        let octal = format!("{sum:o}");
        let mut digits =
            std::iter::once(b'\0').chain(octal.bytes().rev().chain(std::iter::repeat(b'0')));
        for slot in header[148..156].iter_mut().rev() {
            *slot = digits.next().unwrap();
        }
    }

    fn long_name_bomb(declared: u64, provided: usize) -> Vec<u8> {
        let mut header = Header::new_gnu();
        header.set_entry_type(EntryType::GNULongName);
        header.set_size(declared);
        header.set_mode(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        let mut tar = header.as_bytes().to_vec();
        tar.extend(std::iter::repeat_n(b'A', provided));
        let pad = (512 - tar.len() % 512) % 512;
        tar.extend(std::iter::repeat_n(0u8, pad));
        tar
    }
}
