use divan::Bencher;
use etar::{EntryPath, Erofs, Kind, Meta, WriteEntry};
use std::io::Cursor;
use tokio::io::AsyncReadExt;

const SMALL_FILES: &[usize] = &[100, 1_000];
const LARGE_FILE: usize = 4 * 1024 * 1024;

fn main() {
    divan::main();
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("benchmark runtime")
}

fn bytes(size: usize, seed: u32) -> Vec<u8> {
    let mut value = seed | 1;
    (0..size)
        .map(|_| {
            value ^= value << 13;
            value ^= value >> 17;
            value ^= value << 5;
            value as u8
        })
        .collect()
}

/// Build one image from `files` and return its length
async fn image(erofs: &Erofs, files: &[(EntryPath, Vec<u8>)]) -> u64 {
    let mut build = erofs.build().await.expect("build");
    for (path, body) in files {
        let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
        let size = body.len() as u64;
        let entry = WriteEntry::file(path.clone(), meta, size, Cursor::new(body.as_slice()));
        build.append(entry.expect("file")).await.expect("append");
    }
    let mut file = build.finish().await.expect("finish");
    let mut head = [0u8; 4096];
    file.read_exact(&mut head).await.expect("superblock");
    file.metadata().await.expect("stat").len()
}

fn bench_files(bencher: Bencher, files: Vec<(EntryPath, Vec<u8>)>) {
    let runtime = runtime();
    let scratch = tempfile::tempdir().expect("scratch");
    let erofs = Erofs::new(scratch.path()).expect("embedded mkfs.erofs");
    let total: usize = files.iter().map(|(_, body)| body.len()).sum();
    bencher
        .counter(divan::counter::BytesCount::new(total))
        .bench_local(|| runtime.block_on(image(&erofs, &files)));
}

#[divan::bench(args = SMALL_FILES, sample_count = 10)]
fn build_small_files(bencher: Bencher, count: usize) {
    let files = (0..count)
        .map(|n| {
            let path = EntryPath::new(format!("d{}/f{n:05}", n % 16)).expect("path");
            (path, bytes(512 + n % 4096, n as u32))
        })
        .collect();
    bench_files(bencher, files);
}

#[divan::bench(sample_count = 10)]
fn build_large_files(bencher: Bencher) {
    let files = (0..8)
        .map(|n| {
            (
                EntryPath::new(format!("f{n}")).expect("path"),
                bytes(LARGE_FILE, n + 1),
            )
        })
        .collect();
    bench_files(bencher, files);
}
