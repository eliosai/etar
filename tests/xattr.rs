//! PAX extended attributes through the public reader and writer

use etar::{EntryPath, Error, Format, Kind, Meta, ReadLimits, Reader, WriteEntry, Writer};
use std::collections::BTreeMap;
use std::io::Cursor;
use tokio::io::AsyncReadExt;
use tokio_tar::{Builder, EntryType, Header};

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

async fn write(format: Format, entries: Vec<(&str, Meta)>) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new(), format);
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
    writer.finish().await.expect("finish").sink
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
    for format in [Format::Tar, Format::TarZstd] {
        let archive = write(format, entries.clone()).await;
        let got = read_xattrs(archive, ReadLimits::default())
            .await
            .expect("read");
        assert_eq!(got, vec![expected(), expected(), expected()]);
    }
}

#[tokio::test]
async fn writer_emits_schily_records_in_name_order() {
    let archive = write(Format::Tar, vec![("f", sample(Kind::File))]).await;
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
    let archive = write(Format::Tar, vec![("f", meta)]).await;
    assert_eq!(archive[156], b'0');
    assert_eq!(archive.len(), 512 + 512 + 1024);
}

fn libarchive_record(key: &str, value: &str) -> Vec<u8> {
    let body = format!(" {key}={value}\n");
    let mut digits = 1;
    let len = loop {
        let total = body.len() + digits;
        if total.to_string().len() == digits {
            break total;
        }
        digits += 1;
    };
    format!("{len}{body}").into_bytes()
}

async fn libarchive_tar(records: Vec<u8>) -> Vec<u8> {
    let mut builder = Builder::new(Vec::new());
    let mut pax = Header::new_ustar();
    pax.set_entry_type(EntryType::XHeader);
    pax.set_path("PaxHeader/f").expect("pax path");
    pax.set_mode(0o644);
    pax.set_size(records.len() as u64);
    pax.set_cksum();
    builder.append(&pax, &records[..]).await.expect("pax");
    let mut file = Header::new_gnu();
    file.set_entry_type(EntryType::Regular);
    file.set_mode(0o644);
    file.set_uid(0);
    file.set_gid(0);
    file.set_mtime(0);
    file.set_size(2);
    builder
        .append_data(&mut file, "f", &b"hi"[..])
        .await
        .expect("file");
    builder.into_inner().await.expect("finish")
}

#[tokio::test]
async fn libarchive_records_decode_name_and_value() {
    let mut records = libarchive_record("LIBARCHIVE.xattr.user.a%20b%3Dc", "AAH//g==");
    records.extend(libarchive_record("SCHILY.xattr.user.plain", "text"));
    let archive = libarchive_tar(records).await;
    let got = read_xattrs(archive, ReadLimits::default())
        .await
        .expect("read");
    let mut want = Xattrs::new();
    want.insert(b"user.a b=c".to_vec(), vec![0x00, 0x01, 0xFF, 0xFE]);
    want.insert(b"user.plain".to_vec(), b"text".to_vec());
    assert_eq!(got, vec![want]);
}

#[tokio::test]
async fn malformed_libarchive_records_are_malformed() {
    for (key, value) in [
        ("LIBARCHIVE.xattr.user.a%2", "AAH//g=="),
        ("LIBARCHIVE.xattr.user.a", "AAH//g="),
        ("LIBARCHIVE.xattr.user.a", "A!H//g=="),
        ("LIBARCHIVE.xattr.", "AAH//g=="),
        ("LIBARCHIVE.xattr.user.a%00b", "AAH//g=="),
    ] {
        let archive = libarchive_tar(libarchive_record(key, value)).await;
        let result = read_xattrs(archive, ReadLimits::default()).await;
        assert!(matches!(result, Err(Error::Malformed)), "{key} {value}");
    }
}

#[tokio::test]
async fn oversized_pax_record_hits_the_metadata_limit() {
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File).with_xattr("user.big", vec![b'x'; 4096]);
    let archive = write(Format::Tar, vec![("f", meta)]).await;
    let limits = ReadLimits::default().with_max_metadata_bytes(1024);
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
    let archive = write(Format::Tar, vec![("f", meta)]).await;
    let limits = ReadLimits::default().with_max_xattr_bytes(91);
    let result = read_xattrs(archive.clone(), limits).await;
    assert!(matches!(
        result,
        Err(Error::LimitExceeded("max_xattr_bytes"))
    ));
    let limits = ReadLimits::default().with_max_xattr_bytes(92);
    assert!(read_xattrs(archive, limits).await.is_ok());
}

#[tokio::test]
async fn unrepresentable_xattrs_are_rejected_at_construction() {
    let cases = [
        ("", b"v".to_vec()),
        ("user.a=b", b"v".to_vec()),
        ("user.a\0b", b"v".to_vec()),
        ("user.empty", Vec::new()),
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

#[tokio::test]
async fn libarchive_base64_handles_every_padding_length() {
    let mut records = Vec::new();
    for (name, value) in [("one", "QQ=="), ("two", "QUI="), ("three", "QUJD")] {
        records.extend(libarchive_record(
            &format!("LIBARCHIVE.xattr.user.{name}"),
            value,
        ));
    }
    let archive = libarchive_tar(records).await;
    let got = read_xattrs(archive, ReadLimits::default())
        .await
        .expect("read");
    let mut want = Xattrs::new();
    want.insert(b"user.one".to_vec(), b"A".to_vec());
    want.insert(b"user.two".to_vec(), b"AB".to_vec());
    want.insert(b"user.three".to_vec(), b"ABC".to_vec());
    assert_eq!(got, vec![want]);
}
