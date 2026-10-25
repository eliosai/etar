//! EROFS image builds checked by the fsck.erofs built beside the embedded mkfs.erofs extracting them
#![cfg(feature = "erofs")]

use etar::{EntryPath, Erofs, ErofsBuild, Error, Kind, Meta, WriteEntry};
use std::io::Cursor;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn erofs(scratch: &Path) -> Erofs {
    Erofs::new(scratch).expect("erofs")
}

fn noise(len: usize, seed: u32) -> Vec<u8> {
    (0..len as u32)
        .map(|i| (i.wrapping_add(seed).wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect()
}

async fn file(build: &mut ErofsBuild, name: &str, mode: u32, body: Vec<u8>) {
    let meta = Meta::new(mode, 0, 0, 0, Kind::File);
    let size = body.len() as u64;
    let path = EntryPath::new(name).expect("path");
    let entry = WriteEntry::file(path, meta, size, Cursor::new(body)).expect("file");
    build.append(entry).await.expect("append");
}

async fn meta(build: &mut ErofsBuild, name: &str, mode: u32, kind: Kind) {
    let path = EntryPath::new(name).expect("path");
    let entry = WriteEntry::metadata(path, Meta::new(mode, 0, 0, 0, kind)).expect("meta");
    build.append(entry).await.expect("append");
}

/// A tree with empty, small, compressible, and incompressible files and every link kind
async fn sample(build: &mut ErofsBuild) {
    meta(build, "d", 0o755, Kind::Dir).await;
    meta(build, "d/e", 0o700, Kind::Dir).await;
    file(build, "d/empty", 0o644, Vec::new()).await;
    file(build, "d/e/small", 0o600, b"small body".to_vec()).await;
    file(build, "text", 0o644, b"repeat ".repeat(50_000)).await;
    file(build, "noise", 0o755, noise(3 * 1024 * 1024 + 17, 7)).await;
    let target = PathBuf::from("d/e/small");
    meta(
        build,
        "link",
        0o777,
        Kind::Symlink {
            target: target.clone(),
        },
    )
    .await;
    meta(build, "hard", 0o600, Kind::Hardlink { target }).await;
}

async fn sample_image(scratch: &Path) -> Vec<u8> {
    let mut build = erofs(scratch).build().await.expect("build");
    sample(&mut build).await;
    let mut image = build.finish().await.expect("finish");
    let mut bytes = Vec::new();
    image.read_to_end(&mut bytes).await.expect("read image");
    bytes
}

fn extract(image: &[u8]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("image.erofs");
    std::fs::write(&path, image).expect("write image");
    let out = dir.path().join("x");
    let fsck = Command::new(env!("ETAR_FSCK_EROFS"))
        .arg(format!("--extract={}", out.display()))
        .arg("--no-preserve-owner")
        .arg(&path)
        .output()
        .expect("fsck.erofs");
    assert!(
        fsck.status.success(),
        "{}",
        String::from_utf8_lossy(&fsck.stderr)
    );
    dir
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .expect("stat")
        .permissions()
        .mode()
        & 0o7777
}

fn scratch_files(scratch: &Path) -> usize {
    std::fs::read_dir(scratch).expect("scratch").count()
}

#[tokio::test]
async fn an_image_extracts_to_the_streamed_tree() {
    let scratch = tempfile::tempdir().expect("scratch");
    let dir = extract(&sample_image(scratch.path()).await);
    let x = dir.path().join("x");
    let read = |name: &str| std::fs::read(x.join(name)).expect(name);
    assert_eq!(read("d/empty"), b"");
    assert_eq!(read("d/e/small"), b"small body");
    assert_eq!(read("text"), b"repeat ".repeat(50_000));
    assert_eq!(read("noise"), noise(3 * 1024 * 1024 + 17, 7));
    assert_eq!(
        std::fs::read_link(x.join("link")).expect("link"),
        Path::new("d/e/small")
    );
    let hard = std::fs::metadata(x.join("hard")).expect("hard");
    assert_eq!(
        hard.ino(),
        std::fs::metadata(x.join("d/e/small")).expect("small").ino()
    );
    assert_eq!(
        (mode(&x.join("d/e")), mode(&x.join("noise"))),
        (0o700, 0o755)
    );
    assert_eq!(mode(&x.join("d/e/small")), 0o600);
}

#[tokio::test]
async fn the_same_stream_builds_the_same_image() {
    let scratch = tempfile::tempdir().expect("scratch");
    let first = sample_image(scratch.path()).await;
    assert_eq!(first, sample_image(scratch.path()).await);
    let text = b"repeat ".repeat(50_000).len() + 3 * 1024 * 1024;
    assert!(first.len() < text, "{} bytes", first.len());
}

#[tokio::test]
async fn implied_parents_are_created() {
    let scratch = tempfile::tempdir().expect("scratch");
    let mut build = erofs(scratch.path()).build().await.expect("build");
    file(&mut build, "a/b/c", 0o644, b"deep".to_vec()).await;
    let mut image = Vec::new();
    let mut file = build.finish().await.expect("finish");
    file.read_to_end(&mut image).await.expect("read");
    let dir = extract(&image);
    assert_eq!(
        std::fs::read(dir.path().join("x/a/b/c")).expect("c"),
        b"deep"
    );
}

#[tokio::test]
async fn scratch_holds_no_file_during_or_after_a_build() {
    let scratch = tempfile::tempdir().expect("scratch");
    let mut build = erofs(scratch.path()).build().await.expect("build");
    sample(&mut build).await;
    assert_eq!(scratch_files(scratch.path()), 0);
    let image = build.finish().await.expect("finish");
    assert!(image.metadata().await.expect("stat").len() > 0);
    assert_eq!(scratch_files(scratch.path()), 0);
}

#[tokio::test]
async fn dropping_a_build_kills_mkfs_and_leaves_scratch_empty() {
    let scratch = tempfile::tempdir().expect("scratch");
    let erofs = erofs(scratch.path());
    for _ in 0..8 {
        let mut build = erofs.build().await.expect("build");
        file(&mut build, "half", 0o644, noise(1024 * 1024, 1)).await;
        assert_eq!(settled_mkfs(1).await, 1);
        drop(build);
    }
    assert_eq!(settled_mkfs(0).await, 0);
    assert_eq!(scratch_files(scratch.path()), 0);
}

/// Poll live mkfs.erofs children for up to five seconds until `want` remain
async fn settled_mkfs(want: usize) -> usize {
    let started = std::time::Instant::now();
    while running_mkfs() != want && started.elapsed().as_secs() < 5 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    running_mkfs()
}

/// Live mkfs.erofs children of this test process, named by their argv[0]
fn running_mkfs() -> usize {
    let me = std::process::id().to_string();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return 0;
    };
    procs
        .flatten()
        .filter(|p| {
            std::fs::read(p.path().join("cmdline"))
                .is_ok_and(|cmd| cmd.starts_with(b"mkfs.erofs\0"))
        })
        .filter_map(|p| std::fs::read_to_string(p.path().join("stat")).ok())
        .filter(|stat| !stat.contains(") Z "))
        .filter(|stat| stat.split(") ").nth(1).and_then(|s| s.split(' ').nth(1)) == Some(&me))
        .count()
}

#[tokio::test]
async fn a_truncated_stream_never_yields_an_image() {
    let scratch = tempfile::tempdir().expect("scratch");
    let mut build = erofs(scratch.path()).build().await.expect("build");
    file(&mut build, "whole", 0o644, b"kept".to_vec()).await;
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    let path = EntryPath::new("short").expect("path");
    let short = WriteEntry::file(path, meta, 100, Cursor::new(vec![0u8; 10])).expect("file");
    assert!(build.append(short).await.is_err());
    assert!(matches!(build.finish().await, Err(Error::Aborted)));
}

#[tokio::test]
async fn mkfs_fails_on_a_stream_truncated_past_the_pipe_buffer() {
    let scratch = tempfile::tempdir().expect("scratch");
    let mut build = erofs(scratch.path()).build().await.expect("build");
    file(&mut build, "whole", 0o644, noise(256 * 1024, 2)).await;
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    let path = EntryPath::new("short").expect("path");
    let body = Cursor::new(noise(256 * 1024, 4));
    let short = WriteEntry::file(path, meta, 1024 * 1024, body).expect("file");
    let appended = build.append(short).await;
    assert!(
        matches!(appended, Err(Error::BodyLengthMismatch)),
        "{appended:?}"
    );
    let result = build.finish().await;
    assert!(
        matches!(&result, Err(Error::Erofs { status, .. }) if status.code() == Some(1)),
        "{result:?}"
    );
    assert_eq!(scratch_files(scratch.path()), 0);
}

#[tokio::test]
async fn a_dropped_append_mid_body_never_yields_an_image() {
    let scratch = tempfile::tempdir().expect("scratch");
    let mut build = erofs(scratch.path()).build().await.expect("build");
    let (body, mut sender) = tokio::io::duplex(256 * 1024);
    sender
        .write_all(&noise(128 * 1024, 5))
        .await
        .expect("prefix");
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    let path = EntryPath::new("stalled").expect("path");
    let entry = WriteEntry::file(path, meta, 1024 * 1024, body).expect("file");
    let wait = std::time::Duration::from_millis(200);
    let stalled = tokio::time::timeout(wait, build.append(entry)).await;
    assert!(stalled.is_err(), "{stalled:?}");
    let result = build.finish().await;
    assert!(
        matches!(&result, Err(Error::Erofs { status, .. }) if status.code() == Some(1)),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_failed_mkfs_surfaces_its_status_and_stderr() {
    let scratch = tempfile::tempdir().expect("scratch");
    let mut build = erofs(scratch.path()).build().await.expect("build");
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    let body = noise(4 * 1024 * 1024, 3);
    let path = EntryPath::new("n".repeat(300)).expect("path");
    let entry = WriteEntry::file(path, meta, body.len() as u64, Cursor::new(body)).expect("file");
    let result = match build.append(entry).await {
        Ok(()) => build.finish().await.map(drop),
        Err(error) => Err(error),
    };
    let Err(Error::Erofs { status, stderr }) = result else {
        panic!("{result:?}");
    };
    assert_eq!(status.code(), Some(1));
    assert!(stderr.contains("File name too long"), "{stderr}");
}

#[tokio::test]
async fn a_failure_after_the_tar_closes_is_reported_by_finish() {
    let scratch = tempfile::tempdir().expect("scratch");
    let mut build = erofs(scratch.path()).build().await.expect("build");
    file(&mut build, "f", 0o644, b"x".to_vec()).await;
    let absent = Kind::Hardlink {
        target: "absent".into(),
    };
    meta(&mut build, "h", 0o644, absent).await;
    let result = build.finish().await;
    assert!(
        matches!(&result, Err(Error::Erofs { stderr, .. }) if stderr.contains("No such file or directory")),
        "{result:?}"
    );
    assert_eq!(scratch_files(scratch.path()), 0);
}

/// How long one image of `files` empty files in a single directory takes to build
async fn flat(scratch: &Path, files: usize) -> Duration {
    let started = Instant::now();
    let mut build = erofs(scratch).build().await.expect("build");
    for n in 0..files {
        file(&mut build, &format!("d/f{n:07}"), 0o644, Vec::new()).await;
    }
    build.finish().await.expect("finish");
    started.elapsed()
}

#[tokio::test]
async fn one_large_directory_builds_in_linear_time() {
    let scratch = tempfile::tempdir().expect("scratch");
    let small = flat(scratch.path(), 20_000).await;
    let large = flat(scratch.path(), 160_000).await;
    let ratio = large.as_secs_f64() / small.as_secs_f64();
    assert!(
        ratio < 24.0,
        "8x the entries took {ratio:.1}x the time: {small:?} then {large:?}"
    );
}

/// Append `meta` under `name`, with a one-byte body when it is a file
async fn stamped(build: &mut ErofsBuild, name: &str, meta: Meta) {
    let path = EntryPath::new(name).expect("path");
    let result = match meta.kind {
        Kind::File => {
            let entry = WriteEntry::file(path, meta, 1, Cursor::new(b"x")).expect("file");
            build.append(entry).await
        }
        _ => {
            build
                .append(WriteEntry::metadata(path, meta).expect("meta"))
                .await
        }
    };
    result.expect("append");
}

#[tokio::test]
async fn modes_mtimes_and_links_survive_the_image() {
    let scratch = tempfile::tempdir().expect("scratch");
    let mut build = erofs(scratch.path()).build().await.expect("build");
    let target = PathBuf::from(format!("{}/{}", "d".repeat(70), "t".repeat(79)));
    let symlink = Kind::Symlink {
        target: target.clone(),
    };
    let entries = [
        ("d", 0o1755, 1_600_000_000, 7, Kind::Dir),
        ("d/f", 0o4755, 1_700_000_000, 123_456_789, Kind::File),
        ("g", 0o2711, 1_700_000_001, 999_999_999, Kind::File),
        ("d/sym", 0o777, 1_700_000_002, 5, symlink),
    ];
    for (name, mode, secs, nanos, kind) in entries.clone() {
        let meta = Meta::new(mode, 0, 0, secs as u64, kind).with_mtime_nanos(nanos);
        stamped(&mut build, name, meta).await;
    }
    let hard = Kind::Hardlink {
        target: "d/f".into(),
    };
    meta(&mut build, "hard", 0o4755, hard).await;
    let mut image = Vec::new();
    let mut file = build.finish().await.expect("finish");
    file.read_to_end(&mut image).await.expect("read");
    let dir = extract(&image);
    let x = dir.path().join("x");
    for (name, mode, secs, nanos, _) in entries {
        let stat = std::fs::symlink_metadata(x.join(name)).expect(name);
        let got = (stat.mode() & 0o7777, stat.mtime(), stat.mtime_nsec());
        assert_eq!(got, (mode, secs, i64::from(nanos)), "{name}");
    }
    assert_eq!(std::fs::read_link(x.join("d/sym")).expect("link"), target);
    let ino = |name: &str| std::fs::metadata(x.join(name)).expect(name).ino();
    assert_eq!(ino("hard"), ino("d/f"));
}
