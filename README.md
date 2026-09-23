# etar

`etar` reads, writes, and validates tar archives over Tokio byte streams. It
supports raw tar and tar.zst.

```rust
use etar::{EntryPath, Format, Kind, Meta, ReadLimits, Reader, WriteEntry, Writer, validate};
use std::io::Cursor;
use tokio::io::AsyncReadExt;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
runtime.block_on(async {
    let mut writer = Writer::new(Vec::new(), Format::TarZstd);
    let path = EntryPath::new("hello.txt")?;
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    let file = WriteEntry::file(path, meta, 5, Cursor::new(b"hello".to_vec()))?;
    writer.append(file).await?;
    let archive = writer.finish().await?.sink;

    let report = validate(Cursor::new(archive.clone()), ReadLimits::default()).await?;
    assert_eq!(report.entries, 1);

    let mut reader = Reader::open(Cursor::new(archive), ReadLimits::default()).await?;
    let mut entry = reader.next_entry().await?.expect("one file");
    let mut body = Vec::new();
    entry.read_to_end(&mut body).await?;
    assert_eq!(entry.header().path.as_bytes(), b"hello.txt");
    assert_eq!(body, b"hello");
    drop(entry);
    assert!(reader.next_entry().await?.is_none());
    Ok::<_, Box<dyn std::error::Error>>(())
})?;
# Ok(())
# }
```

`Reader::next_entry` lends one body at a time. A caller may read or discard a
body before requesting the next entry. Only a final `Ok(None)` verifies the
tar end blocks, trailing padding, and compressed frames; use `validate` when
no entry data is needed. A read, write, or cancelled archive operation aborts it. Never
publish a sink after `Writer::append` or `Writer::finish` fails.

`EntryPath` rejects absolute and parent components, NUL bytes, and empty
names. Non-UTF-8 Unix path bytes are preserved. Link targets are returned as
untrusted metadata. Validate each target against your destination root before
creating a link.

The default limits cap entry count, per-entry size, decoded archive bytes,
metadata-record size, path length, and decompression ratio. Set tighter
`ReadLimits` for a known workload. See [the interface](docs/api.md),
[architecture](docs/architecture.md), and [benchmark notes](docs/benchmarking.md).
