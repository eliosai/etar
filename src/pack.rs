//! Streaming pack of an entry stream into an uncompressed tar sink

use crate::entry::{Kind, Meta, SafePath};
use futures::{Stream, StreamExt};
use pin_project_lite::pin_project;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio_tar::{Builder, EntryType, Header};

/// Why packing an entry stream failed
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PackError {
    /// A file entry arrived without a body
    #[error("missing file body")]
    MissingBody,
    /// The underlying write or a body read failed
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// One entry to pack: metadata, byte length, and a body for files
pub struct PackEntry<B> {
    /// The relative path under which the entry is stored
    pub path: SafePath,
    /// The entry's unix metadata
    pub meta: Meta,
    /// The body length, which the body must yield exactly
    pub size: u64,
    /// The body bytes for a file, absent for other kinds
    pub body: Option<B>,
}

/// Pack an entry stream into an uncompressed tar, returning the finalized sink
pub async fn pack_to_sink<S, B, W>(entries: S, sink: W) -> Result<W, PackError>
where
    S: Stream<Item = Result<PackEntry<B>, PackError>> + Send,
    B: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut builder = Builder::new(sink);
    futures::pin_mut!(entries);
    while let Some(entry) = entries.next().await {
        append(&mut builder, entry?).await?;
    }
    let mut sink = builder.into_inner().await?;
    sink.shutdown().await?;
    Ok(sink)
}

/// Build the header common to every kind from the entry metadata
fn base_header(meta: &Meta) -> Header {
    let mut header = Header::new_gnu();
    header.set_mode(meta.mode);
    header.set_uid(meta.uid as u64);
    header.set_gid(meta.gid as u64);
    header.set_mtime(meta.mtime_unix_secs);
    header
}

/// Append one entry, streaming a file body or emitting a metadata-only record
async fn append<B, W>(builder: &mut Builder<W>, entry: PackEntry<B>) -> Result<(), PackError>
where
    B: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let PackEntry {
        path,
        meta,
        size,
        body,
    } = entry;
    let mut header = base_header(&meta);
    match meta.kind {
        Kind::File => append_file(builder, &mut header, path.as_path(), size, body).await,
        Kind::Dir => {
            append_meta(
                builder,
                &mut header,
                path.as_path(),
                EntryType::Directory,
                None,
            )
            .await
        }
        Kind::Symlink { target } => {
            append_meta(
                builder,
                &mut header,
                path.as_path(),
                EntryType::Symlink,
                Some(target),
            )
            .await
        }
        Kind::Hardlink { target } => {
            append_meta(
                builder,
                &mut header,
                path.as_path(),
                EntryType::Link,
                Some(target),
            )
            .await
        }
    }
}

/// Stream a file body of exactly `size` bytes into the archive
async fn append_file<B, W>(
    builder: &mut Builder<W>,
    header: &mut Header,
    path: &Path,
    size: u64,
    body: Option<B>,
) -> Result<(), PackError>
where
    B: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    header.set_entry_type(EntryType::Regular);
    header.set_size(size);
    let body = body.ok_or(PackError::MissingBody)?;
    builder
        .append_data(header, path, Exact::new(body, size))
        .await?;
    Ok(())
}

/// Append a bodyless entry, setting a link target when present
async fn append_meta<W>(
    builder: &mut Builder<W>,
    header: &mut Header,
    path: &Path,
    kind: EntryType,
    link: Option<PathBuf>,
) -> Result<(), PackError>
where
    W: AsyncWrite + Unpin + Send,
{
    header.set_entry_type(kind);
    header.set_size(0);
    if let Some(target) = link {
        header.set_link_name(&target)?;
    }
    builder
        .append_data(header, path, tokio::io::empty())
        .await?;
    Ok(())
}

pin_project! {
    /// Yields exactly `remaining` bytes, erroring on a short or long body
    struct Exact<R> {
        #[pin]
        inner: R,
        remaining: u64,
    }
}

impl<R> Exact<R> {
    fn new(inner: R, len: u64) -> Self {
        Self {
            inner,
            remaining: len,
        }
    }
}

impl<R: AsyncRead> AsyncRead for Exact<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.project();
        if *this.remaining == 0 {
            return Poll::Ready(Ok(()));
        }
        let before = buf.filled().len();
        let result = this.inner.poll_read(cx, buf);
        let read = (buf.filled().len() - before) as u64;
        if let Poll::Ready(Ok(())) = &result {
            if read == 0 {
                return Poll::Ready(Err(length_mismatch()));
            }
            if read > *this.remaining {
                buf.set_filled(before + *this.remaining as usize);
                return Poll::Ready(Err(length_mismatch()));
            }
            *this.remaining -= read;
        }
        result
    }
}

/// A read error standing for a body that did not match its declared size
fn length_mismatch() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, "body length mismatch")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::SecurityLimits;
    use crate::open::{OpenEntry, open};
    use pretty_assertions::assert_eq;
    use std::io::Cursor;
    use tokio::io::AsyncReadExt;

    fn meta(kind: Kind) -> Meta {
        Meta {
            mode: 0o644,
            uid: 0,
            gid: 0,
            mtime_unix_secs: 0,
            kind,
        }
    }

    fn file_entry(path: &str, body: &[u8]) -> PackEntry<Cursor<Vec<u8>>> {
        PackEntry {
            path: SafePath::new(path).unwrap(),
            meta: meta(Kind::File),
            size: body.len() as u64,
            body: Some(Cursor::new(body.to_vec())),
        }
    }

    async fn pack(entries: Vec<PackEntry<Cursor<Vec<u8>>>>) -> Result<Vec<u8>, PackError> {
        let stream = futures::stream::iter(entries.into_iter().map(Ok));
        pack_to_sink(stream, Vec::new()).await
    }

    async fn open_all(tar: Vec<u8>) -> Vec<(SafePath, Kind, Vec<u8>)> {
        let stream = open(Cursor::new(tar), SecurityLimits::default());
        futures::pin_mut!(stream);
        let mut out = Vec::new();
        while let Some(item) = stream.next().await {
            out.push(read_entry(item.unwrap()).await);
        }
        out
    }

    async fn read_entry(mut entry: OpenEntry) -> (SafePath, Kind, Vec<u8>) {
        let path = entry.path.clone();
        let kind = entry.meta.kind.clone();
        let mut body = Vec::new();
        entry.read_to_end(&mut body).await.unwrap();
        (path, kind, body)
    }

    #[tokio::test]
    async fn test_pack_open_round_trips_a_file() {
        let tar = pack(vec![file_entry("bin/sh", b"hello")]).await.unwrap();
        let got = open_all(tar).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0.as_path(), Path::new("bin/sh"));
        assert_eq!(got[0].1, Kind::File);
        assert_eq!(got[0].2, b"hello");
    }

    #[tokio::test]
    async fn test_pack_output_is_uncompressed_tar() {
        let tar = pack(vec![file_entry("bin/sh", b"hello")]).await.unwrap();
        // The ustar magic at offset 257 proves a raw tar, not a zstd frame
        assert_eq!(&tar[257..262], b"ustar");
    }

    #[tokio::test]
    async fn test_pack_open_round_trips_mixed_kinds() {
        let entries = vec![
            file_entry("etc/hosts", b"127.0.0.1 localhost"),
            PackEntry {
                path: SafePath::new("usr").unwrap(),
                meta: meta(Kind::Dir),
                size: 0,
                body: None,
            },
            PackEntry {
                path: SafePath::new("bin/python").unwrap(),
                meta: meta(Kind::Symlink {
                    target: "/usr/bin/python3".into(),
                }),
                size: 0,
                body: None,
            },
        ];
        let tar = pack(entries).await.unwrap();
        let got = open_all(tar).await;
        assert_eq!(got.len(), 3);
        assert_eq!(got[1].1, Kind::Dir);
        assert_eq!(
            got[2].1,
            Kind::Symlink {
                target: "/usr/bin/python3".into()
            }
        );
    }

    #[tokio::test]
    async fn test_short_body_is_rejected() {
        let entry = PackEntry {
            path: SafePath::new("f").unwrap(),
            meta: meta(Kind::File),
            size: 10,
            body: Some(Cursor::new(b"hello".to_vec())),
        };
        assert!(matches!(pack(vec![entry]).await, Err(PackError::Io(_))));
    }

    #[tokio::test]
    async fn test_file_without_body_is_rejected() {
        let entry: PackEntry<Cursor<Vec<u8>>> = PackEntry {
            path: SafePath::new("f").unwrap(),
            meta: meta(Kind::File),
            size: 0,
            body: None,
        };
        assert!(matches!(
            pack(vec![entry]).await,
            Err(PackError::MissingBody)
        ));
    }

    #[tokio::test]
    async fn test_large_file_round_trips() {
        let body: Vec<u8> = (0..500 * 1024).map(|i| (i % 251) as u8).collect();
        let tar = pack(vec![file_entry("big.bin", &body)]).await.unwrap();
        let got = open_all(tar).await;
        assert_eq!(got[0].2, body);
    }

    #[tokio::test]
    async fn test_metadata_round_trips() {
        let expected = Meta {
            mode: 0o755,
            uid: 1234,
            gid: 5678,
            mtime_unix_secs: 1_700_000_000,
            kind: Kind::File,
        };
        let entry = PackEntry {
            path: SafePath::new("bin/sh").unwrap(),
            meta: expected.clone(),
            size: 3,
            body: Some(Cursor::new(b"abc".to_vec())),
        };
        let tar = pack(vec![entry]).await.unwrap();
        let stream = open(Cursor::new(tar), SecurityLimits::default());
        futures::pin_mut!(stream);
        let opened = stream.next().await.unwrap().unwrap();
        assert_eq!(opened.meta, expected);
    }

    #[tokio::test]
    async fn test_multi_entry_terminates_cleanly() {
        let entries = vec![
            file_entry("a", b"AAAA"),
            file_entry("b", b"BBBB"),
            file_entry("c", b"CCCC"),
        ];
        let tar = pack(entries).await.unwrap();
        let got = open_all(tar).await;
        assert_eq!(got.len(), 3);
        assert_eq!(got[2].2, b"CCCC");
    }
}
