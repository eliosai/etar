# Interface

The public operations use the same entry vocabulary:

| Operation | Input | Completion |
| --- | --- | --- |
| `Writer::new(sink, format)` | An `AsyncWrite` sink and `Tar` or `TarZstd` | `finish()` returns the finalized sink and encoded byte count |
| `Writer::with_zstd(sink, zstd)` | An `AsyncWrite` sink and a checked `Zstd` | Writes tar.zst at that level and worker count; `finish()` completes it |
| `Reader::open(source, limits)` | An `AsyncRead` source | `next_entry()` yields one borrowed body; `Ok(None)` means the whole archive passed trailer validation |
| `validate(source, limits)` | An `AsyncRead` source | Returns entry and byte counts after consuming every body and the trailer |

`Zstd::new(level, workers)` accepts levels `-131072..=22` (`0` selects zstd's
default) and `0..=256` workers; anything else returns
`Error::InvalidZstdLevel` or `Error::InvalidZstdWorkers`. Zero workers
compress on the calling task. A positive count hands compression to that many
zstd threads, which pays off for large archives. `Writer::new(sink,
Format::TarZstd)` uses `Zstd::default()`, level 3 with no workers. Any level
or worker count produces a standard zstd frame that `Reader` opens.

`Meta::mtime_nanos` holds the nanoseconds past `mtime_unix_secs` and must stay
below one billion; `Meta::with_mtime_nanos` sets it, and the writer returns
`Error::InvalidMtimeNanos` otherwise. The writer emits a PAX
`mtime=<secs>.<9-digit nanos>` record only for a non-zero fraction, so
whole-second entries keep the plain tar layout. The reader prefers a PAX
`mtime`, with or without a fraction of any digit count (extra digits beyond
nine are dropped), and falls back to the ustar header mtime. A negative or
non-numeric PAX `mtime` is `Error::Malformed`.

`Meta::xattrs` holds extended attributes, name bytes to value bytes, ordered by
name. `Meta::with_xattr` sets one. The writer emits each as a PAX
`SCHILY.xattr.<name>=<value>` record in a PAX header entry ahead of the entry,
and entries without attributes get no PAX header. `WriteEntry::file` and
`WriteEntry::metadata` return `Error::InvalidXattr` for an empty name, a name
containing `=` or NUL, or an empty value, because the PAX parser underneath
cannot read an empty value back. The reader surfaces `SCHILY.xattr.*` records
and `LIBARCHIVE.xattr.<percent-encoded name>=<base64 value>` records; other PAX
keys are ignored, and a malformed encoding is `Error::Malformed`.

`WriteEntry::file` requires `Kind::File`, a declared size, and a finite body.
`WriteEntry::metadata` accepts a directory, link, device, or fifo and has no
body. It returns `Error::InvalidDevice` for a device major or minor number
above 2097151, the most a header's seven octal digits hold. The
writer checks for both short and surplus file bytes. A failed append poisons
the writer; `finish` then returns `Error::Aborted`. Cancelling an in-progress
append also poisons it. Dropping an unfinished
writer leaves a partial archive in its sink. Callers must not publish it.

`ReadEntry::header()` borrows the validated path, Unix metadata, and declared
size. Its `AsyncRead` implementation streams the body. Dropping an unread
body lets the reader skip it on the next call. A body read error poisons the
reader. `Reader` confirms completion after it checks both tar end blocks,
trailing padding, and the zstd decoder.
The zstd decoder accepts consecutive frames but rejects trailing garbage.
Cancelling `next_entry()` poisons the reader because parser progress may be
partial.

`EntryPath::new` rejects empty, absolute, parent-ascending, or NUL-bearing
paths. It preserves non-UTF-8 bytes on Unix. `.` names the in-tree root.
Treat link targets in `Kind::Symlink` and `Kind::Hardlink` as untrusted bytes.
Validate them against your destination root before creating links.

`ReadLimits` defaults to 2 million entries, 8 GiB per entry, 32 GiB of
decoded tar bytes, 1 MiB per GNU/PAX extension record, 4096 path bytes,
64 KiB of extended attribute names and values per entry, and a 200:1
decompression ratio. The decoded-byte cap includes tar headers
and padding. The ratio guard starts after 8 MiB of decoded output. Links
may be disabled independently. Extension size is checked before the parser
buffers its payload. Tighten these limits to match the caller's workload.
Regular files, directories, symlinks, and hardlinks are supported. Character
devices, block devices, and fifos surface as `Kind::CharDevice`,
`Kind::BlockDevice`, and `Kind::Fifo` only under
`ReadLimits::with_special_files(true)`; by default they are
`Error::DisallowedEntryType`. Sparse and global PAX entries are rejected.
Directory, link, device, and fifo entries with nonzero body lengths are
rejected.

Transport failures remain `Error::Io` with their original I/O kind. Header
and trailer failures are `Error::Malformed`; unsafe paths and limit breaches
have named variants. Direct body reads use `AsyncRead` and therefore return
`io::Error`. `validate` maps a body limit breach back to
`Error::LimitExceeded`.
