#![doc = include_str!("../README.md")]

mod compress;
mod entry;
#[cfg(feature = "erofs")]
mod erofs;
mod error;
mod guard;
mod limits;
mod mtime;
mod open;
mod pack;
mod pax;
mod reader;
mod settings;
mod writer;
mod xattr;

#[doc(inline)]
pub use entry::{EntryPath, Kind, Meta};
#[cfg(feature = "erofs")]
#[doc(inline)]
pub use erofs::{Erofs, ErofsBuild};
#[doc(inline)]
pub use error::Error;
#[doc(inline)]
pub use limits::ReadLimits;
#[doc(inline)]
pub use reader::{EntryHeader, ReadEntry, Reader};
#[doc(inline)]
pub use writer::{WriteEntry, Writer};
