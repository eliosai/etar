//! Character device, block device, and fifo entries at the public reader and writer

use etar::{EntryPath, Error, Format, Kind, Meta, ReadLimits, Reader, WriteEntry, Writer};
use std::io::Cursor;

async fn write(format: Format, kind: Kind) -> Result<Vec<u8>, Error> {
    let mut writer = Writer::new(Vec::new(), format);
    let meta = Meta::new(0o620, 0, 5, 9, kind);
    let entry = WriteEntry::metadata(EntryPath::new("dev/x").expect("path"), meta)?;
    writer.append(entry).await?;
    Ok(writer.finish().await?.sink)
}

async fn read_kind(archive: Vec<u8>, limits: ReadLimits) -> Result<Kind, Error> {
    let mut reader = Reader::open(Cursor::new(archive), limits).await?;
    let kind = reader
        .next_entry()
        .await?
        .expect("one entry")
        .header()
        .meta
        .kind
        .clone();
    assert!(reader.next_entry().await?.is_none());
    Ok(kind)
}

fn kinds() -> [Kind; 4] {
    [
        Kind::CharDevice { major: 1, minor: 3 },
        Kind::BlockDevice {
            major: 259,
            minor: 1_048_575,
        },
        Kind::CharDevice { major: 0, minor: 0 },
        Kind::Fifo,
    ]
}

#[tokio::test]
async fn special_entries_round_trip_when_allowed() {
    for format in [Format::Tar, Format::TarZstd] {
        for kind in kinds() {
            let archive = write(format, kind.clone()).await.expect("write");
            let limits = ReadLimits::default().with_special_files(true);
            assert_eq!(read_kind(archive, limits).await.expect("read"), kind);
        }
    }
}

#[tokio::test]
async fn special_entries_are_rejected_by_default() {
    for kind in kinds() {
        let archive = write(Format::Tar, kind).await.expect("write");
        let got = read_kind(archive, ReadLimits::default()).await;
        assert!(matches!(got, Err(Error::DisallowedEntryType)), "{got:?}");
    }
}

#[tokio::test]
async fn a_device_number_past_the_header_field_is_refused_on_write() {
    let kind = Kind::CharDevice {
        major: 1 << 21,
        minor: 0,
    };
    let got = write(Format::Tar, kind).await;
    assert!(matches!(got, Err(Error::InvalidDevice)), "{got:?}");
}
