# Interface

The public operations use the same entry vocabulary:

| Operation | Input | Completion |
| --- | --- | --- |
| `Writer::tar(sink)` | An `AsyncWrite` sink | `finish()` writes the tar end blocks, shuts the sink down, and returns it |
| `Writer::tar_zst(sink)` | An `AsyncWrite` sink | As `tar`, inside one zstd frame at level 3 compressed on the calling task |
| `Reader::open(source, limits)` | An `AsyncRead` source, plain or zstd | `next_entry()` yields one borrowed body; `Ok(None)` means the whole archive passed trailer validation |

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
`WriteEntry::metadata` return `Error::InvalidXattr` for an empty name or a name
containing `=` or NUL. PAX reads an empty value as a deletion, so the writer
leaves out an attribute whose value is empty. The reader surfaces
`SCHILY.xattr.*` records and ignores every other PAX key; an empty name is
`Error::Malformed`.

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
paths. It preserves non-UTF-8 bytes on Unix. `.` names the in-tree root, and
the reader refuses a non-directory entry at the root as `Error::Malformed`.
Treat link targets in `Kind::Symlink` and `Kind::Hardlink` as untrusted bytes.
Validate them against your destination root before creating links. The writer
stores a target longer than the 100-byte ustar field in a PAX `linkpath`
record.

`ReadLimits` defaults to 2 million entries, 8 GiB per entry, 32 GiB of decoded
tar bytes, 4096 bytes per entry path or link target, and 64 KiB of extended
attribute names and values per entry. The decoded-byte cap includes tar headers
and padding. Every GNU or PAX extension record stops at 1 MiB, checked before
the parser buffers its payload. Regular files, directories, symlinks,
hardlinks, character devices, block devices, and fifos surface as their `Kind`.
Sparse members, including PAX `GNU.sparse.*` records, global PAX headers, and
other entry types are `Error::DisallowedEntryType`. Directory, link, device,
and fifo entries with nonzero body lengths are rejected.

Transport failures remain `Error::Io` with their original I/O kind. Header
and trailer failures are `Error::Malformed`; unsafe paths and limit breaches
have named variants. Direct body reads use `AsyncRead` and therefore return
`io::Error`.
