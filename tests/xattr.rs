//! PAX extended attributes through the public reader and writer

use etar::{EntryPath, Error, Kind, Meta, ReadLimits, Reader, WriteEntry};
use std::collections::BTreeMap;
use std::io::Cursor;
use tokio::io::AsyncReadExt;
mod common;
use common::{pax_record, pax_tar};

type Xattrs = BTreeMap<Vec<u8>, Vec<u8>>;

fn binary_value() -> Vec<u8> {
    vec![0x00, b'a', 0xFF, 0xFE, b'\n', b'=']
}

fn sample(kind: Kind) -> Meta {
    Meta::new(0o644, 1, 2, 3, kind)
        .with_xattr("user.binary", binary_value())
        .with_xattr("security.capability", b"cap".to_vec())
        .with_xattr("user.name", b"etar".to_vec())
}

async fn write(zstd: bool, entries: Vec<(&str, Meta)>) -> Vec<u8> {
    let mut writer = common::writer(zstd);
    for (name, meta) in entries {
        let path = EntryPath::new(name).expect("path");
        if matches!(meta.kind, Kind::File) {
            let entry = WriteEntry::file(path, meta, 2, Cursor::new(b"hi".to_vec()));
            writer.append(entry.expect("file")).await.expect("append");
        } else {
            let entry = WriteEntry::metadata(path, meta).expect("metadata");
            writer.append(entry).await.expect("append");
        }
    }
    writer.finish().await.expect("finish")
}

async fn read_xattrs(archive: Vec<u8>, limits: ReadLimits) -> Result<Vec<Xattrs>, Error> {
    let mut reader = Reader::open(Cursor::new(archive), limits).await?;
    let mut out = Vec::new();
    while let Some(mut entry) = reader.next_entry().await? {
        out.push(entry.header().meta.xattrs.clone());
        let mut body = Vec::new();
        entry.read_to_end(&mut body).await?;
    }
    Ok(out)
}

fn expected() -> Xattrs {
    sample(Kind::Dir).xattrs
}

#[tokio::test]
async fn file_dir_and_symlink_xattrs_round_trip() {
    let link = Kind::Symlink { target: "f".into() };
    let entries = vec![
        ("f", sample(Kind::File)),
        ("d", sample(Kind::Dir)),
        ("l", sample(link)),
    ];
    for zstd in [false, true] {
        let archive = write(zstd, entries.clone()).await;
        let got = read_xattrs(archive, ReadLimits::default())
            .await
            .expect("read");
        assert_eq!(got, vec![expected(), expected(), expected()]);
    }
}

#[tokio::test]
async fn an_empty_value_is_written_as_no_record() {
    let meta = Meta::new(0o644, 0, 0, 0, Kind::Dir)
        .with_xattr("user.empty", Vec::new())
        .with_xattr("user.name", b"etar".to_vec());
    let only = Meta::new(0o644, 0, 0, 0, Kind::Dir).with_xattr("user.empty", Vec::new());
    let kept: Xattrs = [(b"user.name".to_vec(), b"etar".to_vec())].into();
    for zstd in [false, true] {
        let archive = write(zstd, vec![("d", meta.clone()), ("e", only.clone())]).await;
        let got = read_xattrs(archive, ReadLimits::default())
            .await
            .expect("read");
        assert_eq!(got, vec![kept.clone(), Xattrs::new()]);
    }
    let plain = write(false, vec![("e", only)]).await;
    assert!(!String::from_utf8_lossy(&plain).contains("SCHILY"));
}

#[tokio::test]
async fn writer_emits_schily_records_in_name_order() {
    let archive = write(false, vec![("f", sample(Kind::File))]).await;
    let text = String::from_utf8_lossy(&archive);
    let capability = text
        .find("SCHILY.xattr.security.capability=cap")
        .expect("capability");
    let binary = text.find("SCHILY.xattr.user.binary=").expect("binary");
    let name = text.find("SCHILY.xattr.user.name=etar").expect("name");
    assert!(capability < binary && binary < name);
}

#[tokio::test]
async fn entry_without_xattrs_carries_no_pax_header() {
    let meta = Meta::new(0o644, 1, 2, 3, Kind::File);
    let archive = write(false, vec![("f", meta)]).await;
    assert_eq!(archive[156], b'0');
    assert_eq!(archive.len(), 512 + 512 + 1024);
}

#[tokio::test]
async fn schily_records_decode_and_other_keys_are_ignored() {
    let mut records = pax_record("SCHILY.xattr.user.a b", "\u{1}\u{7f}");
    records.extend(pax_record("SCHILY.xattr.user.plain", "text"));
    records.extend(pax_record("LIBARCHIVE.xattr.user.ignored", "AAH//g=="));
    let archive = pax_tar(records, 0).await;
    let got = read_xattrs(archive, ReadLimits::default())
        .await
        .expect("read");
    let mut want = Xattrs::new();
    want.insert(b"user.a b".to_vec(), vec![0x01, 0x7F]);
    want.insert(b"user.plain".to_vec(), b"text".to_vec());
    assert_eq!(got, vec![want]);
}

#[tokio::test]
async fn empty_xattr_name_is_malformed() {
    let archive = pax_tar(pax_record("SCHILY.xattr.", "v"), 0).await;
    let result = read_xattrs(archive, ReadLimits::default()).await;
    assert!(matches!(result, Err(Error::Malformed)));
}

#[tokio::test]
async fn oversized_pax_record_hits_the_metadata_limit() {
    let meta =
        Meta::new(0o644, 0, 0, 0, Kind::File).with_xattr("user.big", vec![b'x'; 1024 * 1024]);
    let archive = write(false, vec![("f", meta)]).await;
    let limits = ReadLimits {
        max_xattr_bytes: 2 * 1024 * 1024,
        ..ReadLimits::default()
    };
    let result = read_xattrs(archive, limits).await;
    assert!(matches!(
        result,
        Err(Error::LimitExceeded("max_metadata_bytes"))
    ));
}

#[tokio::test]
async fn xattr_total_over_the_cap_is_refused() {
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File)
        .with_xattr("user.a", vec![b'x'; 40])
        .with_xattr("user.b", vec![b'y'; 40]);
    let archive = write(false, vec![("f", meta)]).await;
    let limits = ReadLimits {
        max_xattr_bytes: 91,
        ..ReadLimits::default()
    };
    let result = read_xattrs(archive.clone(), limits).await;
    assert!(matches!(
        result,
        Err(Error::LimitExceeded("max_xattr_bytes"))
    ));
    let limits = ReadLimits {
        max_xattr_bytes: 92,
        ..ReadLimits::default()
    };
    assert!(read_xattrs(archive, limits).await.is_ok());
}

#[tokio::test]
async fn unrepresentable_xattrs_are_rejected_at_construction() {
    let cases = [
        ("", b"v".to_vec()),
        ("user.a=b", b"v".to_vec()),
        ("user.a\0b", b"v".to_vec()),
    ];
    for (name, value) in cases {
        let meta = Meta::new(0o644, 0, 0, 0, Kind::Dir).with_xattr(name, value);
        let path = EntryPath::new("d").expect("path");
        assert!(matches!(
            WriteEntry::metadata(path, meta),
            Err(Error::InvalidXattr)
        ));
    }
}
