# etar

`etar` reads and writes tar archives over Tokio byte streams. It
supports raw tar and tar.zst.

```rust
use etar::{EntryPath, Kind, Meta, ReadLimits, Reader, WriteEntry, Writer};
use std::io::Cursor;
use tokio::io::AsyncReadExt;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
runtime.block_on(async {
    let mut writer = Writer::tar_zst(Vec::new());
    let path = EntryPath::new("hello.txt")?;
    let meta = Meta::new(0o644, 0, 0, 0, Kind::File);
    let file = WriteEntry::file(path, meta, 5, Cursor::new(b"hello".to_vec()))?;
    writer.append(file).await?;
    let archive = writer.finish().await?;

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
tar end blocks, trailing padding, and compressed frames. A read, write, or
cancelled archive operation aborts it. Never publish a sink after
`Writer::append` or `Writer::finish` fails.

`Writer::tar` writes a plain tar and `Writer::tar_zst` writes one zstd frame at
level 3. `Reader` refuses a zstd window over 16 MiB as `Error::Malformed`, which
rules out large `zstd --long` and `--ultra -20` through `-22` archives. `Meta` carries Unix extended attributes as PAX `SCHILY.xattr.*`
records. Sub-second mtime travels as a PAX `mtime` record. Character devices,
block devices, and fifos carry their device numbers.

The `erofs` feature adds `Erofs`, which streams entries as a tar into an
embedded `mkfs.erofs` and returns the image as an unlinked scratch file. The
crate builds the tool from vendored erofs-utils 1.9.4 with its patches, so a
directory of any size builds in linear time, and needs no installed binary. The
image uses 4 KiB blocks and zstd level 3 in 64 KiB clusters with a clear UUID
and a zero build time, so one entry stream builds the same bytes on any host
page size for one zstd version. Entries keep their modes and nanosecond mtimes. Dropping a build kills
`mkfs.erofs` and leaves no file in scratch. The feature builds for linux-gnu
and runs the tool through `/proc/self/fd`.

etar's own code is MIT. The vendored erofs-utils in `erofs/upstream` is mostly
GPL-2.0+ OR MIT, but `lib/hashmap.c` is GPL-2.0-only and `lib/sha256.c` is
Unlicense, so the crate is `MIT AND GPL-2.0-only AND Unlicense` and the embedded
mkfs.erofs is GPL-2.0.

`EntryPath` rejects absolute and parent components, NUL bytes, and empty
names. Non-UTF-8 Unix path bytes are preserved. Link targets are returned as
untrusted metadata. Validate each target against your destination root before
creating a link.

`ReadLimits` caps entry count, per-entry size, decoded archive bytes, path
length, and extended-attribute bytes; GNU and PAX metadata records stop at
1 MiB. Set tighter limits for a known workload. See [the interface](docs/api.md),
[architecture](docs/architecture.md), and [benchmark notes](docs/benchmarking.md).
