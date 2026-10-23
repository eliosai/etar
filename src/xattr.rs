//! Extended attribute checks and PAX decoding under a byte budget

use crate::entry::Meta;
use crate::error::Error;
use crate::settings::pax::SCHILY_XATTR;
use std::collections::BTreeMap;

/// Extended attributes by name
pub type Xattrs = BTreeMap<Vec<u8>, Vec<u8>>;

/// Reject attributes the SCHILY encoding cannot carry back
pub fn check(meta: &Meta) -> Result<(), Error> {
    for name in meta.xattrs.keys() {
        if name.is_empty() || name.iter().any(|byte| matches!(byte, 0 | b'=')) {
            return Err(Error::InvalidXattr);
        }
    }
    Ok(())
}

/// Take a PAX record into `xattrs` when its key names an attribute, charging `budget`
pub fn record(
    xattrs: &mut Xattrs,
    budget: &mut u64,
    key: &[u8],
    value: &[u8],
) -> Result<(), Error> {
    let Some(name) = key.strip_prefix(SCHILY_XATTR) else {
        return Ok(());
    };
    if name.is_empty() || name.contains(&0) {
        return Err(Error::Malformed);
    }
    *budget = budget
        .checked_sub((name.len() + value.len()) as u64)
        .ok_or(Error::LimitExceeded("max_xattr_bytes"))?;
    xattrs.insert(name.to_vec(), value.to_vec());
    Ok(())
}
