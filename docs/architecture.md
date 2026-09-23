# Architecture

You supply `AsyncRead` and `AsyncWrite` adapters for your source and sink.
`etar` handles archive framing, compression, entry validation, and resource
limits.

`Writer` turns checked `WriteEntry` values into tar records. It verifies the
declared length of every file body and finalizes tar and zstd before handing
the sink back. `Reader` detects zstd magic, meters encoded and decoded
bytes, guards GNU/PAX metadata sizes, and lends one validated entry body at
a time. After the parser reaches a tar end block, the reader consumes the
remaining decoded stream and checks the second end block and zero padding.
`validate` uses the same reader and discards file bodies.

`entry` owns relative paths, kinds, and Unix metadata. `limits` owns the
input ceilings. `guard` checks raw tar header sizes before the parser can
allocate an extension buffer. `open` contains the bounded body and byte
meters; `compress` selects the decoder. `pack` emits one tar entry, while
`writer` controls format and finalization.

The `object_store` adapter test lives in `tests/`. The AFL target feeds
bounded input to `validate`. Divan benchmarks exercise the public reader,
writer, and validator APIs.
