//! zstd level and worker selection at the public writer boundary

use etar::{EntryPath, Error, Format, Kind, Meta, ReadLimits, Reader, WriteEntry, Writer, Zstd};
use std::io::Cursor;
use tokio::io::AsyncReadExt;

fn body() -> Vec<u8> {
    (0..600_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect()
}

async fn append_sample<W>(writer: &mut Writer<W>, body: &[u8])
where
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    for name in ["a/one.bin", "a/two.bin"] {
        let path = EntryPath::new(name).expect("path");
        let meta = Meta::new(0o644, 1, 2, 3, Kind::File);
        let size = body.len() as u64;
        let entry = WriteEntry::file(path, meta, size, Cursor::new(body.to_vec())).expect("file");
        writer.append(entry).await.expect("append");
    }
}

async fn read_all(archive: Vec<u8>) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut reader = Reader::open(Cursor::new(archive), ReadLimits::default())
        .await
        .expect("reader");
    let mut out = Vec::new();
    while let Some(mut entry) = reader.next_entry().await.expect("next") {
        let path = entry.header().path.as_bytes().to_vec();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).await.expect("body");
        out.push((path, bytes));
    }
    out
}

async fn write_with(zstd: Zstd) -> Vec<u8> {
    let mut writer = Writer::with_zstd(Vec::new(), zstd);
    append_sample(&mut writer, &body()).await;
    writer.finish().await.expect("finish").sink
}

#[tokio::test]
async fn chosen_levels_and_workers_round_trip_to_identical_entries() {
    let expected = read_all(write_with(Zstd::default()).await).await;
    for (level, workers) in [(1, 0), (1, 4), (19, 2), (-5, 1)] {
        let zstd = Zstd::new(level, workers).expect("valid options");
        let archive = write_with(zstd).await;
        assert_eq!(archive[..4], [0x28, 0xB5, 0x2F, 0xFD]);
        assert_eq!(
            read_all(archive).await,
            expected,
            "level {level} workers {workers}"
        );
    }
}

#[tokio::test]
async fn default_options_match_format_tar_zstd() {
    let mut writer = Writer::new(Vec::new(), Format::TarZstd);
    append_sample(&mut writer, &body()).await;
    let plain = writer.finish().await.expect("finish").sink;
    assert_eq!(write_with(Zstd::default()).await, plain);
}

#[tokio::test]
async fn higher_level_compresses_smaller() {
    let fast = write_with(Zstd::new(1, 0).expect("level 1")).await;
    let slow = write_with(Zstd::new(19, 0).expect("level 19")).await;
    assert!(slow.len() <= fast.len());
}

async fn small_files(format: Format) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new(), format);
    for n in 0..1000 {
        let path = EntryPath::new(format!("d/file-{n:05}")).expect("path");
        let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
        let body = b"a small body every file repeats".to_vec();
        let size = body.len() as u64;
        let entry = WriteEntry::file(path, meta, size, Cursor::new(body)).expect("file");
        writer.append(entry).await.expect("append");
    }
    writer.finish().await.expect("finish").sink
}

#[tokio::test]
async fn many_small_files_compress_like_one_stream_of_the_tar() {
    use async_compression::tokio::write::ZstdEncoder;
    use tokio::io::AsyncWriteExt;
    let mut whole = ZstdEncoder::new(Vec::new());
    whole
        .write_all(&small_files(Format::Tar).await)
        .await
        .expect("write");
    whole.shutdown().await.expect("shutdown");
    let (whole, archive) = (whole.into_inner(), small_files(Format::TarZstd).await);
    assert!(
        archive.len() <= whole.len() * 2,
        "{} bytes against {} for one stream",
        archive.len(),
        whole.len()
    );
}

#[test]
fn out_of_range_level_is_a_typed_error() {
    assert!(matches!(Zstd::new(23, 0), Err(Error::InvalidZstdLevel(23))));
    assert!(matches!(
        Zstd::new(-131_073, 0),
        Err(Error::InvalidZstdLevel(-131_073))
    ));
    assert!(Zstd::new(22, 0).is_ok());
    assert!(Zstd::new(-131_072, 0).is_ok());
}

#[test]
fn worker_count_beyond_the_bound_is_a_typed_error() {
    assert!(matches!(
        Zstd::new(3, 257),
        Err(Error::InvalidZstdWorkers(257))
    ));
    assert!(Zstd::new(3, 256).is_ok());
}
