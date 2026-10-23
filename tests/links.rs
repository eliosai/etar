//! Symlink and hardlink targets at the public reader and writer

use etar::{EntryPath, Error, Kind, Meta, ReadLimits, Reader, WriteEntry};
use std::io::Cursor;
use std::path::PathBuf;
use std::process::Command;

mod common;

fn long_target() -> PathBuf {
    let target = format!("{}/{}", "d".repeat(70), "t".repeat(79));
    assert_eq!(target.len(), 150);
    target.into()
}

fn links() -> [(&'static str, Kind); 2] {
    [
        (
            "sym",
            Kind::Symlink {
                target: long_target(),
            },
        ),
        (
            "hard",
            Kind::Hardlink {
                target: long_target(),
            },
        ),
    ]
}

async fn write_links(zstd: bool) -> Vec<u8> {
    let mut writer = common::writer(zstd);
    for (name, kind) in links() {
        let meta = Meta::new(0o777, 0, 0, 0, kind);
        let entry = WriteEntry::metadata(EntryPath::new(name).expect("path"), meta);
        writer.append(entry.expect("link")).await.expect("append");
    }
    writer.finish().await.expect("finish")
}

async fn read_kinds(archive: Vec<u8>) -> Vec<Kind> {
    read_limited(archive, ReadLimits::default())
        .await
        .expect("read")
}

async fn read_limited(archive: Vec<u8>, limits: ReadLimits) -> Result<Vec<Kind>, Error> {
    let mut reader = Reader::open(Cursor::new(archive), limits).await?;
    let mut kinds = Vec::new();
    while let Some(entry) = reader.next_entry().await? {
        kinds.push(entry.header().meta.kind.clone());
    }
    Ok(kinds)
}

#[tokio::test]
async fn long_link_targets_round_trip() {
    let want: Vec<Kind> = links().into_iter().map(|(_, kind)| kind).collect();
    for zstd in [false, true] {
        assert_eq!(read_kinds(write_links(zstd).await).await, want);
    }
}

#[tokio::test]
async fn gnu_tar_reads_long_link_targets() {
    let dir = tempfile::tempdir().expect("tempdir");
    let archive = dir.path().join("links.tar");
    std::fs::write(&archive, write_links(false).await).expect("write archive");
    let output = Command::new("tar")
        .arg("-tvf")
        .arg(&archive)
        .output()
        .expect("tar");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listing = String::from_utf8(output.stdout).expect("listing");
    let target = long_target().display().to_string();
    let lines: Vec<&str> = listing.lines().collect();
    assert_eq!(lines.len(), 2, "{listing}");
    assert!(lines[0].ends_with(&format!("sym -> {target}")), "{listing}");
    assert!(
        lines[1].ends_with(&format!("hard link to {target}")),
        "{listing}"
    );
}

#[tokio::test]
async fn a_target_that_fits_the_header_needs_no_pax_record() {
    let target = PathBuf::from("t".repeat(100));
    let kind = Kind::Symlink { target };
    let meta = Meta::new(0o777, 0, 0, 0, kind.clone());
    let mut writer = common::writer(false);
    let entry = WriteEntry::metadata(EntryPath::new("sym").expect("path"), meta);
    writer.append(entry.expect("link")).await.expect("append");
    let archive = writer.finish().await.expect("finish");
    assert_eq!(archive[156], b'2');
    assert_eq!(archive.len(), 512 + 1024);
    assert_eq!(read_kinds(archive).await, [kind]);
}

#[tokio::test]
async fn a_link_target_over_max_path_len_is_refused() {
    for (max_path_len, accepted) in [(149, false), (150, true)] {
        let limits = ReadLimits {
            max_path_len,
            ..ReadLimits::default()
        };
        let got = read_limited(write_links(false).await, limits).await;
        match accepted {
            true => assert_eq!(got.expect("read").len(), 2),
            false => assert!(
                matches!(got, Err(Error::LimitExceeded("max_path_len"))),
                "{got:?}"
            ),
        }
    }
}
