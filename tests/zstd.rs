//! tar.zst output at the public writer boundary

use etar::{EntryPath, Kind, Meta, ReadLimits, Reader, WriteEntry, Writer};
use std::io::Cursor;
use tokio::io::AsyncReadExt;

mod common;

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

#[tokio::test]
async fn tar_zst_round_trips_to_the_tar_entries() {
    let mut tar = Writer::tar(Vec::new());
    append_sample(&mut tar, &body()).await;
    let tar = tar.finish().await.expect("finish");
    let mut zst = Writer::tar_zst(Vec::new());
    append_sample(&mut zst, &body()).await;
    let zst = zst.finish().await.expect("finish");
    assert_eq!(zst[..4], [0x28, 0xB5, 0x2F, 0xFD]);
    assert!(zst.len() < tar.len());
    assert_eq!(read_all(zst).await, read_all(tar).await);
}

#[tokio::test]
async fn tar_zst_output_is_deterministic() {
    let mut first = Writer::tar_zst(Vec::new());
    append_sample(&mut first, &body()).await;
    let mut second = Writer::tar_zst(Vec::new());
    append_sample(&mut second, &body()).await;
    let first = first.finish().await.expect("finish");
    assert_eq!(first, second.finish().await.expect("finish"));
}

async fn small_files(zstd: bool) -> Vec<u8> {
    let mut writer = common::writer(zstd);
    for n in 0..1000 {
        let path = EntryPath::new(format!("d/file-{n:05}")).expect("path");
        let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
        let body = b"a small body every file repeats".to_vec();
        let size = body.len() as u64;
        let entry = WriteEntry::file(path, meta, size, Cursor::new(body)).expect("file");
        writer.append(entry).await.expect("append");
    }
    writer.finish().await.expect("finish")
}

#[tokio::test]
async fn many_small_files_compress_like_one_stream_of_the_tar() {
    use async_compression::tokio::write::ZstdEncoder;
    use tokio::io::AsyncWriteExt;
    let mut whole = ZstdEncoder::new(Vec::new());
    whole
        .write_all(&small_files(false).await)
        .await
        .expect("write");
    whole.shutdown().await.expect("shutdown");
    let (whole, archive) = (whole.into_inner(), small_files(true).await);
    assert!(
        archive.len() <= whole.len() * 2,
        "{} bytes against {} for one stream",
        archive.len(),
        whole.len()
    );
}

async fn zstd_with_window(window_log: i32, bytes: &[u8]) -> Vec<u8> {
    use async_compression::{Level, tokio::write::ZstdEncoder, zstd::CParameter};
    use tokio::io::AsyncWriteExt;
    let params = [CParameter::window_log(window_log as u32)];
    let mut encoder = ZstdEncoder::with_quality_and_params(Vec::new(), Level::Default, &params);
    encoder.write_all(bytes).await.expect("write");
    encoder.shutdown().await.expect("shutdown");
    encoder.into_inner()
}

#[tokio::test]
async fn a_window_above_16_mib_is_refused_before_decoding() {
    let tar = small_files(false).await;
    let wide = zstd_with_window(25, &tar).await;
    let result = common::drain(wide, ReadLimits::default()).await;
    assert!(matches!(result, Err(etar::Error::Malformed)), "{result:?}");
    let fits = zstd_with_window(24, &tar).await;
    assert_eq!(
        common::drain(fits, ReadLimits::default())
            .await
            .expect("read"),
        1000
    );
}
