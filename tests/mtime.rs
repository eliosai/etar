//! Sub-second mtime through PAX at the public reader and writer

use etar::{EntryPath, Error, Kind, Meta, ReadLimits, Reader, WriteEntry};
use std::io::Cursor;
use tokio::io::AsyncReadExt;
use tokio_tar::{Builder, EntryType, Header};

mod common;
use common::{pax_record, pax_tar};

async fn write(zstd: bool, meta: Meta) -> Vec<u8> {
    let mut writer = common::writer(zstd);
    let path = EntryPath::new("f").expect("path");
    let entry = WriteEntry::file(path, meta, 2, Cursor::new(b"hi".to_vec())).expect("file");
    writer.append(entry).await.expect("append");
    writer.finish().await.expect("finish")
}

async fn read_mtime(archive: Vec<u8>) -> Result<(u64, u32), Error> {
    let mut reader = Reader::open(Cursor::new(archive), ReadLimits::default()).await?;
    let mut entry = reader.next_entry().await?.expect("one entry");
    let meta = entry.header().meta;
    let mtime = (meta.mtime_unix_secs, meta.mtime_nanos);
    entry.read_to_end(&mut Vec::new()).await?;
    Ok(mtime)
}

fn file_meta(secs: u64) -> Meta {
    Meta::new(0o644, 7, 8, secs, Kind::File)
}

#[tokio::test]
async fn nonzero_fraction_round_trips() {
    for zstd in [false, true] {
        let meta = file_meta(1_700_000_000).with_mtime_nanos(123_456_789);
        let archive = write(zstd, meta).await;
        assert_eq!(
            read_mtime(archive).await.expect("read"),
            (1_700_000_000, 123_456_789)
        );
    }
}

#[tokio::test]
async fn small_fraction_keeps_leading_zeros() {
    let archive = write(false, file_meta(5).with_mtime_nanos(7)).await;
    assert!(String::from_utf8_lossy(&archive).contains("mtime=5.000000007\n"));
    assert_eq!(read_mtime(archive).await.expect("read"), (5, 7));
}

#[tokio::test]
async fn whole_second_output_matches_the_plain_tar_layout() {
    let mut header = Header::new_gnu();
    header.set_mode(0o644);
    header.set_uid(7);
    header.set_gid(8);
    header.set_mtime(1_700_000_000);
    header.set_entry_type(EntryType::Regular);
    header.set_size(2);
    let mut builder = Builder::new(Vec::new());
    builder
        .append_data(&mut header, "f", &b"hi"[..])
        .await
        .expect("append");
    let expected = builder.into_inner().await.expect("finish");
    let actual = write(false, file_meta(1_700_000_000)).await;
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn pax_mtime_tolerates_short_and_long_fractions() {
    let cases = [
        ("1700000000.5", (1_700_000_000, 500_000_000)),
        ("1700000000.123456", (1_700_000_000, 123_456_000)),
        ("1700000000.1234567891", (1_700_000_000, 123_456_789)),
        ("1700000000", (1_700_000_000, 0)),
    ];
    for (value, expected) in cases {
        let archive = pax_tar(pax_record("mtime", value), 99).await;
        assert_eq!(
            read_mtime(archive).await.expect("read"),
            expected,
            "{value}"
        );
    }
}

#[tokio::test]
async fn header_mtime_applies_without_a_pax_mtime() {
    let archive = pax_tar(pax_record("comment", "x"), 99).await;
    assert_eq!(read_mtime(archive).await.expect("read"), (99, 0));
}

#[tokio::test]
async fn unparsable_pax_mtime_is_malformed() {
    for value in [
        "abc",
        "-5",
        "-5.5",
        "1.",
        ".5",
        "1.5x",
        "1.-5",
        "99999999999999999999.0",
    ] {
        let archive = pax_tar(pax_record("mtime", value), 0).await;
        let result = read_mtime(archive).await;
        assert!(matches!(result, Err(Error::Malformed)), "{value}");
    }
}

#[tokio::test]
async fn nanos_at_one_second_are_rejected_at_construction() {
    let path = EntryPath::new("f").expect("path");
    let meta = file_meta(0).with_mtime_nanos(1_000_000_000);
    let result = WriteEntry::file(path, meta, 0, Cursor::new(Vec::new()));
    assert!(matches!(result, Err(Error::InvalidMtimeNanos)));
    let meta = file_meta(0).with_mtime_nanos(999_999_999);
    let path = EntryPath::new("f").expect("path");
    assert!(WriteEntry::file(path, meta, 0, Cursor::new(Vec::new())).is_ok());
}

#[tokio::test]
async fn mtime_and_xattrs_read_back_together() {
    let meta = file_meta(10)
        .with_mtime_nanos(1)
        .with_xattr("user.a", b"v".to_vec());
    let archive = write(false, meta).await;
    let mut reader = Reader::open(Cursor::new(archive), ReadLimits::default())
        .await
        .expect("open");
    let entry = reader.next_entry().await.expect("next").expect("entry");
    assert_eq!(entry.header().meta.mtime_nanos, 1);
    assert_eq!(entry.header().meta.xattrs.len(), 1);
}
