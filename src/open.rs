//! Validation of one untrusted entry and the decoded-byte cap

use crate::entry::{EntryPath, Kind, Meta};
use crate::error::Error;
use crate::limits::ReadLimits;
use crate::pax;
use pin_project_lite::pin_project;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};
use tokio_tar::{Entry, EntryType};

/// Validate one entry's path, metadata, and size, which also bounds what the parser yields
pub async fn open_entry<R>(
    entry: &mut Entry<R>,
    limits: &ReadLimits,
) -> Result<(EntryPath, Meta, u64), Error>
where
    R: AsyncRead + Unpin,
{
    let path = EntryPath::new(within(&entry.path().map_err(malformed)?, limits)?)?;
    let kind = classify(entry, limits)?;
    if !matches!(kind, Kind::Dir) && path.components().next().is_none() {
        return Err(Error::Malformed);
    }
    let size = entry.header().size().map_err(malformed)?;
    if !matches!(kind, Kind::File) && size != 0 {
        return Err(Error::NonFilePayload);
    }
    if size > limits.max_entry_bytes {
        return Err(Error::LimitExceeded("max_entry_bytes"));
    }
    let meta = read_meta(entry, limits, kind).await?;
    Ok((path, meta, size))
}

/// Refuse a path or link target longer than the path length limit
fn within<'a>(path: &'a Path, limits: &ReadLimits) -> Result<&'a Path, Error> {
    match path.as_os_str().len() > limits.max_path_len {
        true => Err(Error::LimitExceeded("max_path_len")),
        false => Ok(path),
    }
}

/// Map the tar entry type to a kind, rejecting dangerous types
fn classify<R>(entry: &Entry<R>, limits: &ReadLimits) -> Result<Kind, Error>
where
    R: AsyncRead + Unpin,
{
    match entry.header().entry_type() {
        EntryType::Regular | EntryType::Continuous => Ok(Kind::File),
        EntryType::Directory => Ok(Kind::Dir),
        EntryType::Symlink => link(entry, limits).map(|target| Kind::Symlink { target }),
        EntryType::Link => link(entry, limits).map(|target| Kind::Hardlink { target }),
        EntryType::Char => device(entry).map(|(major, minor)| Kind::CharDevice { major, minor }),
        EntryType::Block => device(entry).map(|(major, minor)| Kind::BlockDevice { major, minor }),
        EntryType::Fifo => Ok(Kind::Fifo),
        _ => Err(Error::DisallowedEntryType),
    }
}

/// Read a link entry's untrusted target
fn link<R>(entry: &Entry<R>, limits: &ReadLimits) -> Result<PathBuf, Error>
where
    R: AsyncRead + Unpin,
{
    let target = entry
        .link_name()
        .map_err(malformed)?
        .ok_or(Error::Malformed)?;
    Ok(within(&target, limits)?.to_path_buf())
}

/// Read a device entry's major and minor numbers
fn device<R>(entry: &Entry<R>) -> Result<(u32, u32), Error>
where
    R: AsyncRead + Unpin,
{
    let header = entry.header();
    let major = header.device_major().map_err(malformed)?;
    let minor = header.device_minor().map_err(malformed)?;
    major.zip(minor).ok_or(Error::Malformed)
}

/// Read the entry's unix metadata from its header and PAX records
async fn read_meta<R>(entry: &mut Entry<R>, limits: &ReadLimits, kind: Kind) -> Result<Meta, Error>
where
    R: AsyncRead + Unpin,
{
    let extras = pax::read(entry, limits).await?;
    let header = entry.header();
    let (secs, nanos) = match extras.mtime {
        Some(mtime) => mtime,
        None => (header.mtime().map_err(malformed)?, 0),
    };
    Ok(Meta {
        mode: header.mode().map_err(malformed)?,
        uid: u32::try_from(header.uid().map_err(malformed)?).map_err(malformed)?,
        gid: u32::try_from(header.gid().map_err(malformed)?).map_err(malformed)?,
        mtime_unix_secs: secs,
        mtime_nanos: nanos,
        kind,
        xattrs: extras.xattrs,
    })
}

/// Map any parse failure to `Error::Malformed`
fn malformed<E>(_: E) -> Error {
    Error::Malformed
}

pin_project! {
    /// Marks a source read failure as I/O so the reader never reports it as malformed input
    pub struct Tagged<R> {
        #[pin]
        inner: R,
    }
}

impl<R> Tagged<R> {
    pub fn new(inner: R) -> Self {
        Self { inner }
    }
}

impl<R: AsyncRead> AsyncRead for Tagged<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.project().inner.poll_read(cx, buf) {
            Poll::Ready(Err(error)) => {
                Poll::Ready(Err(io::Error::new(error.kind(), Error::Io(error))))
            }
            other => other,
        }
    }
}

pin_project! {
    /// Bounds total decoded bytes mid-decode
    pub struct Cap<R> {
        #[pin]
        inner: R,
        total: Arc<AtomicU64>,
        max: u64,
    }
}

impl<R> Cap<R> {
    pub fn new(inner: R, total: Arc<AtomicU64>, max: u64) -> Self {
        Self { inner, total, max }
    }
}

impl<R: AsyncRead> AsyncRead for Cap<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();
        let before = buf.filled().len();
        let result = this.inner.poll_read(cx, buf);
        let read = (buf.filled().len() - before) as u64;
        if read > 0 && this.total.fetch_add(read, Ordering::Relaxed) + read > *this.max {
            buf.set_filled(before);
            let breach = Error::LimitExceeded("max_total_bytes");
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::InvalidData, breach)));
        }
        result
    }
}

/// Classify a parser read failure as a source failure, a cap breach, or malformed input
pub fn read_error(error: io::Error, total: &AtomicU64, max: u64) -> Error {
    if total.load(Ordering::Relaxed) > max {
        return Error::LimitExceeded("max_total_bytes");
    }
    match error.into_inner().map(|source| source.downcast::<Error>()) {
        Some(Ok(source)) => match *source {
            known @ (Error::Io(_) | Error::LimitExceeded(_) | Error::DisallowedEntryType) => known,
            _ => Error::Malformed,
        },
        _ => Error::Malformed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::Reader;
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

    async fn collect(tar: Vec<u8>, limits: ReadLimits) -> Vec<Result<(EntryPath, Vec<u8>), Error>> {
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
    async fn test_max_entries_is_enforced() {
        let tar = tar_of(&[
            ("a", EntryType::Regular, b"x"),
            ("b", EntryType::Regular, b"y"),
        ])
        .await;
        let got = collect(
            tar,
            ReadLimits {
                max_entries: 1,
                ..ReadLimits::default()
            },
        )
        .await;
        assert!(matches!(
            got.last(),
            Some(Err(Error::LimitExceeded("max_entries")))
        ));
    }

    #[tokio::test]
    async fn test_traversal_entry_is_rejected() {
        let tar = raw_path_entry("../../etc/passwd", b"x");
        let got = collect(tar, ReadLimits::default()).await;
        assert!(
            matches!(got[0], Err(Error::ParentEscape)),
            "{:?}",
            got.first()
        );
    }

    #[tokio::test]
    async fn test_total_decoded_cap_aborts_oversized_body() {
        let tar = tar_of(&[("f", EntryType::Regular, &[b'x'; 4096])]).await;
        let got = collect(
            tar,
            ReadLimits {
                max_total_bytes: 1024,
                ..ReadLimits::default()
            },
        )
        .await;
        assert!(matches!(got[0], Err(Error::Io(_))));
    }

    #[tokio::test]
    async fn test_long_name_header_bomb_is_capped() {
        let tar = long_name_bomb(64 * 1024, 64 * 1024);
        let got = collect(
            tar,
            ReadLimits {
                max_total_bytes: 8 * 1024,
                ..ReadLimits::default()
            },
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
