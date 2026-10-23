//! PAX extended header records: encoded for the writer, read for the opener

use crate::entry::Meta;
use crate::error::Error;
use crate::limits::ReadLimits;
use crate::mtime;
use crate::settings::pax::{MTIME, NANOS_PER_SEC, SCHILY_XATTR};
use crate::xattr::{self, Collector};
use std::collections::BTreeMap;
use tokio::io::AsyncRead;
use tokio_tar::Entry;

/// What an entry's PAX header adds to its ustar header
pub struct Extras {
    /// Extended attributes by name
    pub xattrs: BTreeMap<Vec<u8>, Vec<u8>>,
    /// Modification time seconds and nanoseconds, when the header carries one
    pub mtime: Option<(u64, u32)>,
}

/// Reject metadata the PAX records cannot carry back
pub fn check(meta: &Meta) -> Result<(), Error> {
    if meta.mtime_nanos >= NANOS_PER_SEC {
        return Err(Error::InvalidMtimeNanos);
    }
    xattr::check(meta)
}

/// Encode the PAX body for one entry, empty when the entry needs none
pub fn encode(meta: &Meta) -> Vec<u8> {
    let mut body = Vec::new();
    if meta.mtime_nanos != 0 {
        let value = format!("{}.{:09}", meta.mtime_unix_secs, meta.mtime_nanos);
        push_record(&mut body, MTIME.as_bytes(), value.as_bytes());
    }
    for (name, value) in meta.xattrs.iter().filter(|(_, value)| !value.is_empty()) {
        let key = [SCHILY_XATTR, name.as_slice()].concat();
        push_record(&mut body, &key, value);
    }
    body
}

/// Append one `<length> <key>=<value>\n` record, where length counts itself
fn push_record(body: &mut Vec<u8>, key: &[u8], value: &[u8]) {
    let rest = key.len() + value.len() + 3;
    let mut digits = 1;
    while (rest + digits).to_string().len() != digits {
        digits += 1;
    }
    body.extend_from_slice((rest + digits).to_string().as_bytes());
    body.push(b' ');
    body.extend_from_slice(key);
    body.push(b'=');
    body.extend_from_slice(value);
    body.push(b'\n');
}

/// Read the extended attributes and mtime the entry's PAX header carries
pub async fn read<R>(entry: &mut Entry<R>, limits: &ReadLimits) -> Result<Extras, Error>
where
    R: AsyncRead + Unpin,
{
    let mut collector = Collector::new(limits.max_xattr_bytes);
    let mut mtime = None;
    let extensions = entry.pax_extensions().await.map_err(|_| Error::Malformed)?;
    for extension in extensions.into_iter().flatten() {
        let extension = extension.map_err(|_| Error::Malformed)?;
        if extension.key_bytes() == MTIME.as_bytes() {
            mtime = Some(mtime::parse(extension.value_bytes())?);
        }
        collector.record(extension.key_bytes(), extension.value_bytes())?;
    }
    Ok(Extras {
        xattrs: collector.finish(),
        mtime,
    })
}
