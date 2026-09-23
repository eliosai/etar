//! Archive interoperability with object_store, GNU tar, and mkfs.erofs

use etar::{EntryPath, Format, Kind, Meta, ReadEntry, ReadLimits, Reader, WriteEntry, Writer};
use object_store::buffered::{BufReader, BufWriter};
use object_store::memory::InMemory;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt};
use std::io::Cursor;
use std::process::Command;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWrite};

async fn append_sample<W>(writer: &mut Writer<W>)
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    let path = EntryPath::new("etc/hosts").expect("path");
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    writer
        .append(
            WriteEntry::file(path, meta, 19, Cursor::new(b"127.0.0.1 localhost".to_vec()))
                .expect("file"),
        )
        .await
        .expect("append file");
    let path = EntryPath::new("usr").expect("path");
    let meta = Meta::new(0o755, 0, 0, 0, Kind::Dir);
    writer
        .append(WriteEntry::metadata(path, meta).expect("directory"))
        .await
        .expect("append directory");
    let path = EntryPath::new("usr/bin/app").expect("path");
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    let body = b"\x7fELF binary-ish payload";
    writer
        .append(
            WriteEntry::file(path, meta, body.len() as u64, Cursor::new(body.to_vec()))
                .expect("file"),
        )
        .await
        .expect("append file");
    let path = EntryPath::new("usr/bin/python").expect("path");
    let kind = Kind::Symlink {
        target: "/usr/bin/python3".into(),
    };
    let meta = Meta::new(0o777, 0, 0, 0, kind);
    writer
        .append(WriteEntry::metadata(path, meta).expect("symlink"))
        .await
        .expect("append symlink");
}

async fn pack_sample() -> Vec<u8> {
    let mut writer = Writer::new(Vec::new(), Format::Tar);
    append_sample(&mut writer).await;
    writer.finish().await.expect("finish").sink
}

async fn read_entry(mut entry: ReadEntry<'_>) -> (String, Kind, Vec<u8>) {
    let path = entry.header().path.as_path().to_string_lossy().into_owned();
    let kind = entry.header().meta.kind.clone();
    let mut body = Vec::new();
    entry.read_to_end(&mut body).await.expect("body");
    (path, kind, body)
}

#[tokio::test]
async fn object_store_round_trip() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let path = ObjectPath::from("exports/fs.tar");
    let sink = BufWriter::new(store.clone(), path.clone());
    let mut writer = Writer::new(sink, Format::Tar);
    append_sample(&mut writer).await;
    writer.finish().await.expect("finish");

    let meta = store.head(&path).await.expect("head");
    let source = BufReader::new(store.clone(), &meta);
    let mut reader = Reader::open(source, ReadLimits::default())
        .await
        .expect("reader");
    let mut paths = Vec::new();
    while let Some(entry) = reader.next_entry().await.expect("next") {
        paths.push(read_entry(entry).await.0);
    }
    assert_eq!(paths, ["etc/hosts", "usr", "usr/bin/app", "usr/bin/python"]);
}

#[tokio::test]
async fn gnu_tar_reads_etar_output() {
    let tar = pack_sample().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let archive = dir.path().join("out.tar");
    std::fs::write(&archive, &tar).expect("write fixture");
    let output = Command::new("tar")
        .arg("-tf")
        .arg(&archive)
        .output()
        .expect("tar");
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let listing = String::from_utf8(output.stdout).expect("listing");
    assert!(listing.contains("etc/hosts"));
    assert!(listing.contains("usr/bin/python"));
}

#[tokio::test]
async fn etar_reads_gnu_tar_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("sub")).expect("mkdir");
    std::fs::write(dir.path().join("sub/file.txt"), b"from gnu tar").expect("write");
    let archive = dir.path().join("in.tar");
    let status = Command::new("tar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(dir.path())
        .arg("sub")
        .status()
        .expect("tar");
    assert!(status.success());

    let bytes = std::fs::read(&archive).expect("read archive");
    let mut reader = Reader::open(Cursor::new(bytes), ReadLimits::default())
        .await
        .expect("reader");
    let mut found = false;
    while let Some(entry) = reader.next_entry().await.expect("next") {
        let (path, _, body) = read_entry(entry).await;
        if path == "sub/file.txt" {
            assert_eq!(body, b"from gnu tar");
            found = true;
        }
    }
    assert!(found);
}

#[tokio::test]
#[ignore = "needs erofs-utils (mkfs.erofs)"]
async fn mkfs_erofs_consumes_etar_tar() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tar = dir.path().join("rootfs.tar");
    let mut writer = Writer::new(Vec::new(), Format::Tar);
    let path = EntryPath::new("f").expect("path");
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    writer
        .append(
            WriteEntry::file(path, meta, 10, Cursor::new(b"erofs body".to_vec())).expect("file"),
        )
        .await
        .expect("append");
    std::fs::write(&tar, writer.finish().await.expect("finish").sink).expect("write tar");
    let image = dir.path().join("rootfs.erofs");
    let output = Command::new("mkfs.erofs")
        .arg("--tar=f")
        .arg(&image)
        .arg(&tar)
        .output()
        .expect("mkfs.erofs");
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let extracted = dir.path().join("extracted");
    std::fs::create_dir(&extracted).expect("mkdir");
    let fsck = Command::new("fsck.erofs")
        .arg(format!("--extract={}", extracted.display()))
        .arg("--no-preserve-owner")
        .arg(&image)
        .output()
        .expect("fsck.erofs");
    assert!(
        fsck.status.success(),
        "{}",
        String::from_utf8_lossy(&fsck.stderr)
    );
    assert_eq!(
        std::fs::read(extracted.join("f")).expect("read"),
        b"erofs body"
    );
}
