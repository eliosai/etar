use bytes::Bytes;
use divan::Bencher;
use etar::{EntryPath, Format, Kind, Meta, ReadLimits, Reader, WriteEntry, Writer, Zstd, validate};
use std::io::Cursor;

const SIZES: &[usize] = &[4096, 262_144, 4_194_304];
const ENTRY_COUNTS: &[usize] = &[100, 1_000];
const ZSTD_PAYLOAD: usize = 32 * 1024 * 1024;
/// Level and worker pairs: single-threaded against four workers
const ZSTD_OPTIONS: &[(i32, u32)] = &[(1, 0), (1, 4), (3, 0), (3, 4)];

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

/// Text-like bytes that zstd compresses by roughly three to one
fn compressible(size: usize) -> Bytes {
    let mut value = 0x9e37_79b9u32;
    let mut out = Vec::with_capacity(size + 8);
    while out.len() < size {
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        out.extend_from_slice(
            format!("key{:04} = value{};\n", value % 4096, value % 97).as_bytes(),
        );
    }
    out.truncate(size);
    Bytes::from(out)
}

async fn write_archive(bytes: Bytes, format: Format) -> Vec<u8> {
    write_with(Writer::new(Vec::new(), format), bytes).await
}

async fn write_with(mut writer: Writer<Vec<u8>>, bytes: Bytes) -> Vec<u8> {
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

#[divan::bench(args = ZSTD_OPTIONS)]
fn write_zstd_options(bencher: Bencher, options: &(i32, u32)) {
    let rt = runtime();
    let bytes = compressible(ZSTD_PAYLOAD);
    let zstd = Zstd::new(options.0, options.1).expect("zstd options");
    bencher.bench_local(|| {
        let output = rt.block_on(async {
            let writer = Writer::with_zstd(Vec::new(), zstd);
            write_with(writer, bytes.clone()).await
        });
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
