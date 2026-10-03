//! PAX extended header records: encoded for the writer, read for the opener

use crate::entry::Meta;
use crate::error::Error;
use crate::limits::SecurityLimits;
use crate::settings::pax::SCHILY_XATTR;
use crate::xattr::Collector;
use std::collections::BTreeMap;
use tokio::io::AsyncRead;
use tokio_tar::Entry;

/// Encode the PAX body for one entry, empty when the entry needs none
pub fn encode(meta: &Meta) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in &meta.xattrs {
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

/// Read the extended attributes the entry's PAX header carries
pub async fn read_xattrs<R>(
    entry: &mut Entry<R>,
    limits: &SecurityLimits,
) -> Result<BTreeMap<Vec<u8>, Vec<u8>>, Error>
where
    R: AsyncRead + Unpin,
{
    let mut collector = Collector::new(limits.max_xattr_bytes);
    let extensions = entry.pax_extensions().await.map_err(|_| Error::Malformed)?;
    for extension in extensions.into_iter().flatten() {
        let extension = extension.map_err(|_| Error::Malformed)?;
        collector.record(extension.key_bytes(), extension.value_bytes())?;
    }
    Ok(collector.finish())
}
