//! Tar entry encoding shared by both output formats

use crate::entry::{Kind, Meta};
use crate::error::Error;
use crate::pax;
use crate::settings::device::MAX_NUMBER;
use crate::settings::pax::HEADER_PATH;
use crate::writer::WriteEntry;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio_tar::{Builder, EntryType, Header};

/// Append one entry, refusing a body shorter or longer than its declared length
pub async fn append<B, W>(builder: &mut Builder<W>, entry: WriteEntry<B>) -> Result<(), Error>
where
    B: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let WriteEntry {
        path,
        meta,
        size,
        mut body,
    } = entry;
    append_pax(builder, &meta).await?;
    let mut header = header(&meta, size)?;
    let mut limited = (&mut body).take(size);
    builder
        .append_data(&mut header, path.as_path(), &mut limited)
        .await?;
    if limited.limit() != 0 || body.read(&mut [0u8; 1]).await? != 0 {
        return Err(Error::BodyLengthMismatch);
    }
    Ok(())
}

/// Build the ustar header for an entry of `size` body bytes
fn header(meta: &Meta, size: u64) -> Result<Header, Error> {
    let mut header = Header::new_gnu();
    header.set_mode(meta.mode);
    header.set_uid(u64::from(meta.uid));
    header.set_gid(u64::from(meta.gid));
    header.set_mtime(meta.mtime_unix_secs);
    header.set_size(size);
    let (kind, link) = match &meta.kind {
        Kind::File => (EntryType::Regular, None),
        Kind::Dir => (EntryType::Directory, None),
        Kind::Symlink { target } => (EntryType::Symlink, Some(target)),
        Kind::Hardlink { target } => (EntryType::Link, Some(target)),
        Kind::CharDevice { .. } => (EntryType::Char, None),
        Kind::BlockDevice { .. } => (EntryType::Block, None),
        Kind::Fifo => (EntryType::Fifo, None),
    };
    header.set_entry_type(kind);
    if let Some(target) = link.filter(|target| pax::fits_header(target)) {
        header.set_link_name(target)?;
    }
    if let Kind::CharDevice { major, minor } | Kind::BlockDevice { major, minor } = meta.kind {
        header.set_device_major(major)?;
        header.set_device_minor(minor)?;
    }
    Ok(header)
}

/// Refuse a device number its seven-digit octal header field cannot hold
pub fn check_device(kind: &Kind) -> Result<(), Error> {
    match kind {
        Kind::CharDevice { major, minor } | Kind::BlockDevice { major, minor }
            if *major > MAX_NUMBER || *minor > MAX_NUMBER =>
        {
            Err(Error::InvalidDevice)
        }
        _ => Ok(()),
    }
}

/// Emit the PAX header entry preceding an entry that needs one
async fn append_pax<W>(builder: &mut Builder<W>, meta: &Meta) -> Result<(), Error>
where
    W: AsyncWrite + Unpin + Send,
{
    let body = pax::encode(meta);
    if body.is_empty() {
        return Ok(());
    }
    let mut header = Header::new_ustar();
    header.set_entry_type(EntryType::XHeader);
    header.set_path(HEADER_PATH)?;
    header.set_mode(0o644);
    header.set_size(body.len() as u64);
    header.set_cksum();
    builder.append(&header, &body[..]).await?;
    Ok(())
}
