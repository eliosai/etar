//! Per-entry heap cost for metadata-heavy archives

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use etar::{EntryPath, Format, Kind, Meta, ReadLimits, Reader, WriteEntry, Writer};
use std::io::Cursor;

const ENTRIES: usize = 1_000;

#[tokio::test]
async fn reading_many_empty_files_has_bounded_allocations() {
    let mut writer = Writer::new(Vec::new(), Format::Tar);
    for number in 0..ENTRIES {
        let path = EntryPath::new(format!("files/{number}")).expect("path");
        let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
        let entry = WriteEntry::file(path, meta, 0, Cursor::new(Vec::new())).expect("entry");
        writer.append(entry).await.expect("append");
    }
    let archive = writer.finish().await.expect("finish").sink;

    let profiler = dhat::Profiler::builder().testing().build();
    let mut reader = Reader::open(Cursor::new(archive), ReadLimits::default())
        .await
        .expect("reader");
    let mut count = 0;
    while let Some(entry) = reader.next_entry().await.expect("next") {
        drop(entry);
        count += 1;
    }
    let blocks = dhat::HeapStats::get().total_blocks;
    drop(profiler);
    assert_eq!(count, ENTRIES);
    assert!(
        blocks < 4_500,
        "read {ENTRIES} entries with {blocks} heap blocks"
    );
}
