//! Streaming and failure behavior at the public API boundary

use etar::{
    EntryPath, Error, Format, Kind, Meta, ReadLimits, Reader, WriteEntry, Writer, validate,
};
use std::io::{self, Cursor};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};

struct OneByte {
    bytes: Vec<u8>,
    offset: usize,
}

impl AsyncRead for OneByte {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if out.remaining() != 0 && self.offset < self.bytes.len() {
            out.put_slice(&self.bytes[self.offset..self.offset + 1]);
            self.offset += 1;
        }
        Poll::Ready(Ok(()))
    }
}

struct BrokenSource {
    sent_prefix: bool,
    kind: io::ErrorKind,
    prefix: [u8; 4],
}

impl AsyncRead for BrokenSource {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.sent_prefix {
            return Poll::Ready(Err(io::Error::new(self.kind, "source lost")));
        }
        out.put_slice(&self.prefix);
        self.sent_prefix = true;
        Poll::Ready(Ok(()))
    }
}

async fn one_file(format: Format, body: &[u8]) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new(), format);
    let path = EntryPath::new("file").expect("path");
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    writer
        .append(
            WriteEntry::file(path, meta, body.len() as u64, Cursor::new(body.to_vec()))
                .expect("file"),
        )
        .await
        .expect("append");
    writer.finish().await.expect("finish").sink
}

#[tokio::test]
async fn zstd_magic_detection_survives_one_byte_reads() {
    let source = OneByte {
        bytes: one_file(Format::TarZstd, b"abc").await,
        offset: 0,
    };
    let mut reader = Reader::open(source, ReadLimits::default())
        .await
        .expect("reader");
    let mut entry = reader.next_entry().await.expect("next").expect("entry");
    let mut body = Vec::new();
    entry.read_to_end(&mut body).await.expect("body");
    assert_eq!(body, b"abc");
    drop(entry);
    assert!(reader.next_entry().await.expect("end").is_none());
}

#[tokio::test]
async fn source_errors_keep_their_io_kind() {
    for (kind, prefix) in [
        (io::ErrorKind::BrokenPipe, *b"tar!"),
        (io::ErrorKind::Other, [0x28, 0xb5, 0x2f, 0xfd]),
    ] {
        let source = BrokenSource {
            sent_prefix: false,
            kind,
            prefix,
        };
        let mut reader = Reader::open(source, ReadLimits::default())
            .await
            .expect("prefix");
        let error = match reader.next_entry().await {
            Ok(_) => panic!("broken source was accepted"),
            Err(error) => error,
        };
        assert!(matches!(error, Error::Io(error) if error.kind() == kind));
    }
}

#[tokio::test]
async fn checksum_corruption_is_rejected_before_yield() {
    let mut raw = one_file(Format::Tar, b"x").await;
    raw[0] ^= 1;
    let mut reader = Reader::open(Cursor::new(raw), ReadLimits::default())
        .await
        .expect("reader");
    assert!(matches!(reader.next_entry().await, Err(Error::Malformed)));
    assert!(matches!(reader.next_entry().await, Err(Error::Aborted)));
}

#[tokio::test]
async fn oversized_entry_is_rejected_before_yield() {
    let raw = one_file(Format::Tar, b"0123456789").await;
    let limits = ReadLimits::default().with_max_entry_bytes(4);
    let mut reader = Reader::open(Cursor::new(raw), limits)
        .await
        .expect("reader");
    assert!(matches!(
        reader.next_entry().await,
        Err(Error::LimitExceeded("max_entry_bytes"))
    ));
}

#[tokio::test]
async fn truncated_body_is_not_a_clean_end() {
    let raw = one_file(Format::Tar, &[7u8; 16]).await;
    let truncated = raw[..512 + 8].to_vec();
    assert!(
        validate(Cursor::new(truncated), ReadLimits::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn zstd_trailing_garbage_is_rejected() {
    let mut raw = one_file(Format::TarZstd, b"x").await;
    raw.extend_from_slice(b"garbage");
    assert!(
        validate(Cursor::new(raw), ReadLimits::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cancelled_append_aborts_the_writer() {
    let (body, _sender) = tokio::io::duplex(16);
    let mut writer = Writer::new(Vec::new(), Format::Tar);
    let path = EntryPath::new("file").expect("path");
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    let entry = WriteEntry::file(path, meta, 1, body).expect("entry");
    let mut append = Box::pin(writer.append(entry));
    assert!(futures::poll!(append.as_mut()).is_pending());
    drop(append);
    assert!(matches!(writer.finish().await, Err(Error::Aborted)));
}

#[tokio::test]
async fn cancelled_next_entry_aborts_the_reader() {
    let (source, mut sender) = tokio::io::duplex(16);
    sender.write_all(b"ustar").await.expect("prefix");
    let mut reader = Reader::open(source, ReadLimits::default())
        .await
        .expect("reader");
    let mut next = Box::pin(reader.next_entry());
    assert!(futures::poll!(next.as_mut()).is_pending());
    drop(next);
    assert!(matches!(reader.next_entry().await, Err(Error::Aborted)));
}
