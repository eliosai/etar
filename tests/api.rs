use etar::{
    EntryPath, Error, Format, Kind, Meta, ReadLimits, Reader, WriteEntry, Writer, validate,
};
use std::io::Cursor;
use tokio::io::AsyncReadExt;

#[tokio::test]
async fn writer_and_reader_round_trip_one_file() {
    let mut writer = Writer::new(Vec::new(), Format::Tar);
    let path = EntryPath::new("hello.txt").expect("path");
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    let file = WriteEntry::file(path, meta, 5, Cursor::new(b"hello")).expect("file");
    writer.append(file).await.expect("append");
    let output = writer.finish().await.expect("finish");
    assert_eq!(output.encoded_bytes as usize, output.sink.len());

    let mut reader = Reader::open(Cursor::new(output.sink), ReadLimits::default())
        .await
        .expect("reader");
    let mut entry = reader.next_entry().await.expect("entry").expect("file");
    let mut body = Vec::new();
    entry.read_to_end(&mut body).await.expect("body");
    assert_eq!(entry.header().path.as_bytes(), b"hello.txt");
    assert_eq!(body, b"hello");
    drop(entry);
    assert!(reader.next_entry().await.expect("end").is_none());
}

async fn archive(format: Format, body: &[u8]) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new(), format);
    let path = EntryPath::new("data/file").expect("path");
    let meta = Meta::new(0o640, 12, 34, 56, Kind::File);
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
async fn compressed_archive_round_trips_and_validates() {
    let bytes = archive(Format::TarZstd, b"compressed body").await;
    let report = validate(Cursor::new(bytes.clone()), ReadLimits::default())
        .await
        .expect("valid archive");
    assert_eq!(report.entries, 1);
    assert_eq!(report.encoded_bytes as usize, bytes.len());
    assert!(report.decoded_bytes > report.encoded_bytes);
}

#[tokio::test]
async fn missing_tar_end_blocks_are_rejected() {
    let bytes = archive(Format::Tar, b"body").await;
    for cut in [512, 1024] {
        let truncated = bytes[..bytes.len() - cut].to_vec();
        assert!(
            validate(Cursor::new(truncated), ReadLimits::default())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn nonzero_tar_trailer_is_rejected() {
    let mut bytes = archive(Format::Tar, b"body").await;
    bytes.extend_from_slice(b"hidden payload");
    assert!(matches!(
        validate(Cursor::new(bytes), ReadLimits::default()).await,
        Err(Error::Malformed)
    ));
}

#[tokio::test]
async fn truncated_zstd_footer_is_rejected() {
    let mut bytes = archive(Format::TarZstd, b"body").await;
    bytes.pop();
    assert!(
        validate(Cursor::new(bytes), ReadLimits::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn empty_consecutive_zstd_frame_is_verified() {
    use async_compression::tokio::write::ZstdEncoder;
    use tokio::io::AsyncWriteExt;

    let mut bytes = archive(Format::TarZstd, b"body").await;
    let mut encoder = ZstdEncoder::new(Vec::new());
    encoder.shutdown().await.expect("empty frame");
    bytes.extend_from_slice(&encoder.into_inner());
    let report = validate(Cursor::new(bytes.clone()), ReadLimits::default())
        .await
        .expect("valid consecutive frame");
    assert_eq!(report.encoded_bytes as usize, bytes.len());
}

#[tokio::test]
async fn writer_rejects_mismatched_body_length() {
    for declared in [2, 4] {
        let mut writer = Writer::new(Vec::new(), Format::Tar);
        let path = EntryPath::new("a").expect("path");
        let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
        let entry =
            WriteEntry::file(path, meta, declared, Cursor::new(b"abc".to_vec())).expect("entry");
        assert!(matches!(
            writer.append(entry).await,
            Err(Error::BodyLengthMismatch)
        ));
        assert!(matches!(writer.finish().await, Err(Error::Aborted)));
    }
}

#[tokio::test]
async fn invalid_entry_kind_is_rejected_before_writing() {
    let path = EntryPath::new("dir").expect("path");
    let meta = Meta::new(0o755, 0, 0, 0, Kind::Dir);
    assert!(matches!(
        WriteEntry::file(path, meta, 0, Cursor::new(Vec::new())),
        Err(Error::InvalidEntryKind)
    ));
}

#[tokio::test]
async fn non_file_entries_with_bodies_are_rejected() {
    use tokio_tar::{Builder, EntryType, Header};

    for kind in [EntryType::Directory, EntryType::Symlink, EntryType::Link] {
        let mut builder = Builder::new(Vec::new());
        let mut header = Header::new_gnu();
        header.set_entry_type(kind);
        header.set_size(1);
        header.set_mode(0o755);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        if kind != EntryType::Directory {
            header.set_link_name("target").expect("link target");
        }
        builder
            .append_data(&mut header, "entry", &b"x"[..])
            .await
            .expect("malformed fixture");
        let raw = builder.into_inner().await.expect("archive");
        let mut reader = Reader::open(Cursor::new(raw), ReadLimits::default())
            .await
            .expect("reader");
        assert!(matches!(
            reader.next_entry().await,
            Err(Error::NonFilePayload)
        ));
    }
}

#[tokio::test]
async fn reader_enforces_declared_limits() {
    let bytes = archive(Format::Tar, b"long body").await;
    let limits = ReadLimits::default().with_max_entry_bytes(3);
    assert!(matches!(
        validate(Cursor::new(bytes), limits).await,
        Err(Error::LimitExceeded("max_entry_bytes"))
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn non_utf8_path_bytes_survive_round_trip() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let name = OsStr::from_bytes(b"a/\xff");
    let path = EntryPath::new(name).expect("path");
    let mut writer = Writer::new(Vec::new(), Format::Tar);
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    writer
        .append(WriteEntry::file(path, meta, 1, Cursor::new(b"x".to_vec())).expect("file"))
        .await
        .expect("append");
    let bytes = writer.finish().await.expect("finish").sink;
    let mut reader = Reader::open(Cursor::new(bytes), ReadLimits::default())
        .await
        .expect("reader");
    let entry = reader.next_entry().await.expect("next").expect("entry");
    assert_eq!(entry.header().path.as_bytes(), b"a/\xff");
}

#[tokio::test]
async fn extension_size_is_capped_before_it_is_buffered() {
    use tokio_tar::{EntryType, Header};

    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::GNULongName);
    header.set_size(2 * 1024 * 1024);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    let mut raw = header.as_bytes().to_vec();
    raw.extend_from_slice(&[0u8; 1024]);
    let limits = ReadLimits::default().with_max_metadata_bytes(1024);
    let mut reader = Reader::open(Cursor::new(raw), limits)
        .await
        .expect("reader");
    assert!(matches!(
        reader.next_entry().await,
        Err(Error::LimitExceeded("max_metadata_bytes"))
    ));
}

#[tokio::test]
async fn validator_reports_ratio_limit_as_a_domain_error() {
    let body = vec![0u8; 16 * 1024 * 1024];
    let bytes = archive(Format::TarZstd, &body).await;
    let limits = ReadLimits::default().with_max_compression_ratio(10);
    assert!(matches!(
        validate(Cursor::new(bytes), limits).await,
        Err(Error::LimitExceeded("max_compression_ratio"))
    ));
}

#[tokio::test]
async fn pax_size_override_stays_aligned_with_the_parser() {
    use tokio_tar::{EntryType, Header};

    let mut raw = Vec::new();
    let record = b"10 size=5\n";
    let mut extension = Header::new_ustar();
    extension.set_entry_type(EntryType::XHeader);
    extension.set_path("PaxHeader").expect("path");
    extension.set_size(record.len() as u64);
    extension.set_cksum();
    raw.extend_from_slice(extension.as_bytes());
    raw.extend_from_slice(record);
    raw.resize(1024, 0);

    let mut file = Header::new_ustar();
    file.set_entry_type(EntryType::Regular);
    file.set_path("f").expect("path");
    file.set_size(0);
    file.set_mode(0o644);
    file.set_uid(0);
    file.set_gid(0);
    file.set_mtime(0);
    file.set_cksum();
    raw.extend_from_slice(file.as_bytes());
    raw.extend_from_slice(b"hello");
    raw.resize(2048, 0);
    raw.extend_from_slice(&[0u8; 1024]);

    {
        use futures::StreamExt;
        let mut archive = tokio_tar::Archive::new(Cursor::new(raw.clone()));
        let mut entries = archive.entries().expect("tar entries");
        let mut entry = entries.next().await.expect("raw entry").expect("valid pax");
        let mut body = Vec::new();
        entry.read_to_end(&mut body).await.expect("raw body");
        assert_eq!(body, b"hello");
    }

    let mut reader = Reader::open(Cursor::new(raw), ReadLimits::default())
        .await
        .expect("reader");
    let mut entry = reader.next_entry().await.expect("entry").expect("file");
    let mut body = Vec::new();
    entry.read_to_end(&mut body).await.expect("body");
    assert_eq!(body, b"hello");
    drop(entry);
    assert!(reader.next_entry().await.expect("end").is_none());
}
