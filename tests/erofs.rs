//! EROFS image builds checked by the pinned fsck.erofs extracting them
#![cfg(feature = "erofs")]

use etar::{EntryPath, Erofs, ErofsBuild, Error, Kind, Meta, WriteEntry};
use std::io::Cursor;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use tokio::io::AsyncReadExt;

/// A pinned erofs-utils binary, from `ETAR_EROFS_BIN` or the `just erofs-tools` prefix
fn tool(name: &str) -> PathBuf {
    let dir = std::env::var_os("ETAR_EROFS_BIN").map(PathBuf::from);
    let default = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/tools/erofs-1.9.4/bin");
    let path = dir.unwrap_or(default).join(name);
    assert!(
        path.exists(),
        "{} missing: run `just erofs-tools`",
        path.display()
    );
    path
}

async fn erofs(scratch: &Path) -> Erofs {
    Erofs::new(tool("mkfs.erofs"), scratch)
        .await
        .expect("erofs")
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
    let mut build = erofs(scratch).await.build().expect("build");
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
    let fsck = Command::new(tool("fsck.erofs"))
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
    let mut build = erofs(scratch.path()).await.build().expect("build");
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
    let mut build = erofs(scratch.path()).await.build().expect("build");
    sample(&mut build).await;
    assert_eq!(scratch_files(scratch.path()), 0);
    let image = build.finish().await.expect("finish");
    assert!(image.metadata().await.expect("stat").len() > 0);
    assert_eq!(scratch_files(scratch.path()), 0);
}

#[tokio::test]
async fn dropping_a_build_kills_mkfs_and_leaves_scratch_empty() {
    let scratch = tempfile::tempdir().expect("scratch");
    let erofs = erofs(scratch.path()).await;
    for _ in 0..8 {
        let mut build = erofs.build().expect("build");
        file(&mut build, "half", 0o644, noise(1024 * 1024, 1)).await;
        assert_eq!(running_mkfs(), 1);
        drop(build);
    }
    let started = std::time::Instant::now();
    while running_mkfs() > 0 && started.elapsed().as_secs() < 5 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(running_mkfs(), 0);
    assert_eq!(scratch_files(scratch.path()), 0);
}

/// Live mkfs.erofs children of this test process
fn running_mkfs() -> usize {
    let me = std::process::id().to_string();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return 0;
    };
    procs
        .flatten()
        .filter_map(|p| std::fs::read_to_string(p.path().join("stat")).ok())
        .filter(|stat| stat.contains("(mkfs.erofs)") && !stat.contains(") Z "))
        .filter(|stat| stat.split(") ").nth(1).and_then(|s| s.split(' ').nth(1)) == Some(&me))
        .count()
}

#[tokio::test]
async fn a_truncated_stream_never_yields_an_image() {
    let scratch = tempfile::tempdir().expect("scratch");
    let mut build = erofs(scratch.path()).await.build().expect("build");
    file(&mut build, "whole", 0o644, b"kept".to_vec()).await;
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    let path = EntryPath::new("short").expect("path");
    let short = WriteEntry::file(path, meta, 100, Cursor::new(vec![0u8; 10])).expect("file");
    assert!(build.append(short).await.is_err());
    assert!(matches!(build.finish().await, Err(Error::Aborted)));
}

/// An executable that reports 1.9.4 and runs `body` for any other invocation
fn fake(dir: &Path, body: &str) -> PathBuf {
    fake_version(dir, "1.9.4", body)
}

/// An executable that reports `version` and runs `body` for any other invocation
fn fake_version(dir: &Path, version: &str, body: &str) -> PathBuf {
    let path = dir.join("mkfs.erofs");
    let script = format!(
        "#!/bin/sh\n[ \"$1\" = -V ] && echo 'mkfs.erofs (erofs-utils) {version}' && exit 0\n{body}\n"
    );
    std::fs::write(&path, script).expect("script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

#[tokio::test]
async fn a_failed_mkfs_surfaces_its_status_and_stderr() {
    let dir = tempfile::tempdir().expect("dir");
    let program = fake(dir.path(), "echo 'tar: bad header' >&2; exit 3");
    let erofs = Erofs::new(program, dir.path()).await.expect("erofs");
    let mut build = erofs.build().expect("build");
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    let body = noise(4 * 1024 * 1024, 3);
    let path = EntryPath::new("big").expect("path");
    let entry = WriteEntry::file(path, meta, body.len() as u64, Cursor::new(body)).expect("file");
    let result = match build.append(entry).await {
        Ok(()) => build.finish().await.map(drop),
        Err(error) => Err(error),
    };
    let Err(Error::Erofs { status, stderr }) = result else {
        panic!("{result:?}");
    };
    assert_eq!(
        (status.code(), stderr.as_str()),
        (Some(3), "tar: bad header")
    );
}

#[tokio::test]
async fn a_failure_after_the_tar_closes_is_reported_by_finish() {
    let dir = tempfile::tempdir().expect("dir");
    let program = fake(dir.path(), "cat >/dev/null; echo 'no space' >&2; exit 1");
    let mut build = Erofs::new(program, dir.path())
        .await
        .expect("erofs")
        .build()
        .expect("build");
    file(&mut build, "f", 0o644, b"x".to_vec()).await;
    let result = build.finish().await;
    assert!(
        matches!(&result, Err(Error::Erofs { stderr, .. }) if stderr == "no space"),
        "{result:?}"
    );
}

#[tokio::test]
async fn only_mkfs_erofs_1_9_4_is_accepted() {
    let dir = tempfile::tempdir().expect("dir");
    let result = Erofs::new("/bin/echo", dir.path()).await;
    assert!(matches!(result, Err(Error::ErofsVersion(_))), "{result:?}");
    let missing = Erofs::new(dir.path().join("absent"), dir.path()).await;
    assert!(matches!(missing, Err(Error::Io(_))), "{missing:?}");
    for version in ["1.9.45", "11.9.4", "1.9.4-dirty"] {
        let dir = tempfile::tempdir().expect("dir");
        let program = fake_version(dir.path(), version, "exit 0");
        let result = Erofs::new(program, dir.path()).await;
        let banner = format!("mkfs.erofs (erofs-utils) {version}");
        assert!(
            matches!(&result, Err(Error::ErofsVersion(got)) if *got == banner),
            "{version}: {result:?}"
        );
    }
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
    let mut build = erofs(scratch.path()).await.build().expect("build");
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
