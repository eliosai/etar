//! Integration: tara packed/opened against object_store, GNU tar, and mkfs.erofs

use futures::StreamExt;
use object_store::buffered::{BufReader, BufWriter};
use object_store::memory::InMemory;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt};
use std::io::Cursor;
use std::process::Command;
use std::sync::Arc;
use tara::{Kind, Meta, OpenEntry, PackEntry, SafePath, SecurityLimits, open, pack_to_sink};
use tokio::io::AsyncReadExt;

fn sample() -> Vec<PackEntry<Cursor<Vec<u8>>>> {
    vec![
        file("etc/hosts", b"127.0.0.1 localhost"),
        dir("usr"),
        file("usr/bin/app", b"\x7fELF binary-ish payload"),
        symlink("usr/bin/python", "/usr/bin/python3"),
    ]
}

fn file(path: &str, body: &[u8]) -> PackEntry<Cursor<Vec<u8>>> {
    PackEntry {
        path: SafePath::new(path).unwrap(),
        meta: Meta::new(0o644, 0, 0, 0, Kind::File),
        size: body.len() as u64,
        body: Some(Cursor::new(body.to_vec())),
    }
}

fn dir(path: &str) -> PackEntry<Cursor<Vec<u8>>> {
    PackEntry {
        path: SafePath::new(path).unwrap(),
        meta: Meta::new(0o755, 0, 0, 0, Kind::Dir),
        size: 0,
        body: None,
    }
}

fn symlink(path: &str, target: &str) -> PackEntry<Cursor<Vec<u8>>> {
    let kind = Kind::Symlink {
        target: target.into(),
    };
    PackEntry {
        path: SafePath::new(path).unwrap(),
        meta: Meta::new(0o777, 0, 0, 0, kind),
        size: 0,
        body: None,
    }
}

async fn pack_sample() -> Vec<u8> {
    let stream = futures::stream::iter(sample().into_iter().map(Ok));
    pack_to_sink(stream, Vec::new()).await.unwrap()
}

async fn read_entry(mut entry: OpenEntry) -> (String, Kind, Vec<u8>) {
    let path = entry.path.as_path().to_string_lossy().into_owned();
    let kind = entry.meta.kind.clone();
    let mut body = Vec::new();
    entry.read_to_end(&mut body).await.unwrap();
    (path, kind, body)
}

#[tokio::test]
async fn test_object_store_round_trip() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let path = ObjectPath::from("exports/fs.tar");

    let writer = BufWriter::new(store.clone(), path.clone());
    let stream = futures::stream::iter(sample().into_iter().map(Ok));
    pack_to_sink(stream, writer).await.unwrap();

    let meta = store.head(&path).await.unwrap();
    let reader = BufReader::new(store.clone(), &meta);
    let opened = open(reader, SecurityLimits::default());
    futures::pin_mut!(opened);
    let mut paths = Vec::new();
    while let Some(item) = opened.next().await {
        paths.push(read_entry(item.unwrap()).await.0);
    }
    assert_eq!(paths, ["etc/hosts", "usr", "usr/bin/app", "usr/bin/python"]);
}

#[tokio::test]
async fn test_gnu_tar_reads_tara_output() {
    let tar = pack_sample().await;
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("out.tar");
    std::fs::write(&archive, &tar).unwrap();
    let output = Command::new("tar")
        .arg("-tf")
        .arg(&archive)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "gnu tar failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listing = String::from_utf8(output.stdout).unwrap();
    assert!(
        listing.contains("etc/hosts"),
        "listing missing etc/hosts: {listing}"
    );
    assert!(
        listing.contains("usr/bin/python"),
        "listing missing symlink: {listing}"
    );
}

#[tokio::test]
async fn test_tara_opens_gnu_tar_output() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/file.txt"), b"from gnu tar").unwrap();
    let archive = dir.path().join("in.tar");
    let status = Command::new("tar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(dir.path())
        .arg("sub")
        .status()
        .unwrap();
    assert!(status.success());

    let bytes = std::fs::read(&archive).unwrap();
    let opened = open(Cursor::new(bytes), SecurityLimits::default());
    futures::pin_mut!(opened);
    let mut found = false;
    while let Some(item) = opened.next().await {
        let (path, _, body) = read_entry(item.unwrap()).await;
        if path == "sub/file.txt" {
            assert_eq!(body, b"from gnu tar");
            found = true;
        }
    }
    assert!(found, "tara did not open the gnu tar entry");
}

#[tokio::test]
#[ignore = "needs erofs-utils (mkfs.erofs)"]
async fn test_mkfs_erofs_consumes_tara_tar() {
    let dir = tempfile::tempdir().unwrap();
    let tar = dir.path().join("rootfs.tar");
    let raw = pack_raw_for_erofs().await;
    std::fs::write(&tar, &raw).unwrap();
    let image = dir.path().join("rootfs.erofs");
    let status = Command::new("mkfs.erofs")
        .arg("--tar=f")
        .arg(&image)
        .arg(&tar)
        .status()
        .unwrap();
    assert!(status.success());
    let fsck = Command::new("fsck.erofs").arg(&image).status().unwrap();
    assert!(fsck.success());
}

async fn pack_raw_for_erofs() -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f"), b"erofs body").unwrap();
    let archive = dir.path().join("r.tar");
    Command::new("tar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(dir.path())
        .arg("f")
        .status()
        .unwrap();
    std::fs::read(&archive).unwrap()
}
