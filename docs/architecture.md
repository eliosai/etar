# Architecture

You supply `AsyncRead` and `AsyncWrite` adapters for your source and sink.
`etar` handles archive framing, compression, entry validation, and resource
limits.

`Writer` turns checked `WriteEntry` values into tar records. It verifies the
declared length of every file body and finalizes tar and zstd before handing
the sink back. `Reader` detects zstd magic, caps decoded bytes, guards
GNU/PAX metadata sizes, and lends one validated entry body at a time. After
the parser reaches a tar end block, the reader consumes the remaining decoded
stream and checks the second end block and zero padding.

`entry` owns relative paths, kinds, and Unix metadata. `limits` owns the
input ceilings. `guard` checks raw tar header sizes before the parser can
allocate an extension buffer. `open` validates one entry and holds the
decoded-byte cap; `compress` selects the decoder; `settings` fixes the zstd level and the
metadata-record ceiling. `pack` emits one tar entry, while
`pax`, `xattr`, and `mtime` encode and decode extended attributes under the
`max_xattr_bytes` budget and sub-second modification times.
`writer` controls format and finalization.

The `object_store` adapter test lives in `tests/`. The AFL target feeds
bounded input to `Reader`. Divan benchmarks exercise the public reader and
writer.
