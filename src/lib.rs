#![doc = include_str!("../README.md")]

mod compress;
mod entry;
mod error;
mod guard;
mod limits;
mod open;
mod pack;
mod reader;
mod settings;
mod writer;

#[doc(inline)]
pub use entry::SafePath as EntryPath;
#[doc(inline)]
pub use entry::{Kind, Meta};
#[doc(inline)]
pub use error::Error;
#[doc(inline)]
pub use limits::SecurityLimits as ReadLimits;
#[doc(inline)]
pub use reader::{EntryHeader, ReadEntry, Reader, ValidationReport, validate};
#[doc(inline)]
pub use writer::{Format, WriteEntry, WriteOutput, Writer};
