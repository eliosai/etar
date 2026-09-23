//! Streaming, disk-backed tar pack and open for a content-addressed store
//!
//! tara is sans-IO: it owns no threads, sockets, or object store and never
//! touches the filesystem except the scratch directory a caller hands it
//! Callers drive the transport; tara only transforms byte streams
//!
//! Three primitives cover packing, opening, and content hashing:
//! - [`pack_to_sink`] writes an entry stream into an uncompressed tar sink
//! - [`open`] reads an untrusted archive into validated, byte-bounded entries
//! - [`capture`] spools a body to RAM or NVMe while hashing it with BLAKE3
//!
//! [`open`] is hardened against hostile input: [`SecurityLimits`] bounds entry
//! count, total and per-entry size, path length, and decompression ratio, while
//! [`SafePath`] rejects traversal and absolute paths at the entry boundary
//!
//! Compression is sniffed from the stream magic, so [`open`] transparently
//! handles raw tar, tar.zst, and tar.gz without the caller declaring a codec

mod compress;
mod entry;
mod limits;
mod open;
mod pack;
mod spool;

#[doc(inline)]
pub use entry::{Kind, Meta, PathError, SafePath};
#[doc(inline)]
pub use limits::SecurityLimits;
#[doc(inline)]
pub use open::{OpenEntry, OpenError, open};
#[doc(inline)]
pub use pack::{PackEntry, PackError, pack_to_sink};
#[doc(inline)]
pub use spool::{SpoolError, SpoolOptions, Spooled, SpooledReader, capture};
