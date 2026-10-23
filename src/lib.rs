#![doc = include_str!("../README.md")]

mod compress;
mod entry;
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
#[doc(inline)]
pub use error::Error;
#[doc(inline)]
pub use limits::ReadLimits;
#[doc(inline)]
pub use reader::{EntryHeader, ReadEntry, Reader};
#[doc(inline)]
pub use writer::{WriteEntry, Writer};
