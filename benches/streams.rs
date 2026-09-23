use bytes::Bytes;
use divan::Bencher;
use etar::{EntryPath, Format, Kind, Meta, ReadLimits, Reader, WriteEntry, Writer, validate};
use std::io::Cursor;

const SIZES: &[usize] = &[4096, 262_144, 4_194_304];
const ENTRY_COUNTS: &[usize] = &[100, 1_000];

fn main() {
    divan::main();
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("benchmark runtime")
}

fn payload(size: usize) -> Bytes {
    let mut value = 0x1234_5678u32;
    Bytes::from(
        (0..size)
            .map(|_| {
                value ^= value << 13;
                value ^= value >> 17;
                value ^= value << 5;
                value as u8
            })
            .collect::<Vec<_>>(),
    )
}

async fn write_archive(bytes: Bytes, format: Format) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new(), format);
    let path = EntryPath::new("objects/payload.bin").expect("static path");
    let meta = Meta::new(0o644, 1000, 1000, 0, Kind::File);
    let entry =
        WriteEntry::file(path, meta, bytes.len() as u64, Cursor::new(bytes)).expect("entry");
    writer.append(entry).await.expect("append");
    writer.finish().await.expect("finish").sink
}

async fn read_archive(archive: Bytes) -> u64 {
    let mut reader = Reader::open(Cursor::new(archive), ReadLimits::default())
        .await
        .expect("reader");
    let mut count = 0;
    while let Some(mut entry) = reader.next_entry().await.expect("next") {
        count += tokio::io::copy(&mut entry, &mut tokio::io::sink())
            .await
            .expect("body");
    }
    count
}

async fn write_empty_files(count: usize) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new(), Format::Tar);
    for number in 0..count {
        let path = EntryPath::new(format!("files/{number}")).expect("path");
        let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
        let entry = WriteEntry::file(path, meta, 0, Cursor::new(Vec::new())).expect("entry");
        writer.append(entry).await.expect("append");
    }
    writer.finish().await.expect("finish").sink
}

async fn count_entries(archive: Bytes) -> usize {
    let mut reader = Reader::open(Cursor::new(archive), ReadLimits::default())
        .await
        .expect("reader");
    let mut count = 0;
    while let Some(mut entry) = reader.next_entry().await.expect("next") {
        tokio::io::copy(&mut entry, &mut tokio::io::sink())
            .await
            .expect("body");
        count += 1;
    }
    count
}

#[divan::bench(args = SIZES)]
fn write_tar(bencher: Bencher, size: usize) {
    let rt = runtime();
    let bytes = payload(size);
    bencher.bench_local(|| {
        let output = rt.block_on(write_archive(bytes.clone(), Format::Tar));
        divan::black_box(output.len())
    });
}

#[divan::bench(args = SIZES)]
fn write_zstd(bencher: Bencher, size: usize) {
    let rt = runtime();
    let bytes = payload(size);
    bencher.bench_local(|| {
        let output = rt.block_on(write_archive(bytes.clone(), Format::TarZstd));
        divan::black_box(output.len())
    });
}

#[divan::bench(args = SIZES)]
fn read_tar(bencher: Bencher, size: usize) {
    let rt = runtime();
    let archive = Bytes::from(rt.block_on(write_archive(payload(size), Format::Tar)));
    bencher.bench_local(|| divan::black_box(rt.block_on(read_archive(archive.clone()))));
}

#[divan::bench(args = SIZES)]
fn read_zstd(bencher: Bencher, size: usize) {
    let rt = runtime();
    let archive = Bytes::from(rt.block_on(write_archive(payload(size), Format::TarZstd)));
    bencher.bench_local(|| divan::black_box(rt.block_on(read_archive(archive.clone()))));
}

#[divan::bench(args = SIZES)]
fn validate_tar(bencher: Bencher, size: usize) {
    let rt = runtime();
    let archive = Bytes::from(rt.block_on(write_archive(payload(size), Format::Tar)));
    bencher.bench_local(|| {
        let report = rt
            .block_on(validate(
                Cursor::new(archive.clone()),
                ReadLimits::default(),
            ))
            .expect("validate");
        divan::black_box(report.decoded_bytes)
    });
}

#[divan::bench(args = SIZES)]
fn validate_zstd(bencher: Bencher, size: usize) {
    let rt = runtime();
    let archive = Bytes::from(rt.block_on(write_archive(payload(size), Format::TarZstd)));
    bencher.bench_local(|| {
        let report = rt
            .block_on(validate(
                Cursor::new(archive.clone()),
                ReadLimits::default(),
            ))
            .expect("validate");
        divan::black_box(report.decoded_bytes)
    });
}

#[divan::bench(args = ENTRY_COUNTS)]
fn read_many_entries(bencher: Bencher, count: usize) {
    let rt = runtime();
    let archive = Bytes::from(rt.block_on(write_empty_files(count)));
    bencher.bench_local(|| divan::black_box(rt.block_on(count_entries(archive.clone()))));
}
