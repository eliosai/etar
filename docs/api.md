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

`WriteEntry::file` requires `Kind::File`, a declared size, and a finite body.
`WriteEntry::metadata` accepts a directory or link and has no body. The
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
and a 200:1 decompression ratio. The decoded-byte cap includes tar headers
and padding. The ratio guard starts after 8 MiB of decoded output. Links
may be disabled independently. Extension size is checked before the parser
buffers its payload. Tighten these limits to match the caller's workload.
Regular files, directories, symlinks, and hardlinks are supported. Device,
sparse, and global PAX entries are rejected. Directory and link entries with
nonzero body lengths are rejected.

Transport failures remain `Error::Io` with their original I/O kind. Header
and trailer failures are `Error::Malformed`; unsafe paths and limit breaches
have named variants. Direct body reads use `AsyncRead` and therefore return
`io::Error`. `validate` maps a body limit breach back to
`Error::LimitExceeded`.
