//! Hand-built PAX archives shared by integration tests
#![allow(
    dead_code,
    reason = "each test binary uses a different subset of the shared helpers"
)]

use tokio_tar::{Builder, EntryType, Header};

/// One PAX record with a correct self-counting length
pub fn pax_record(key: &str, value: &str) -> Vec<u8> {
    let body = format!(" {key}={value}\n");
    let mut digits = 1;
    let len = loop {
        let total = body.len() + digits;
        if total.to_string().len() == digits {
            break total;
        }
        digits += 1;
    };
    format!("{len}{body}").into_bytes()
}

/// A tar holding file `f` preceded by a PAX header of `records`, with a ustar mtime
pub async fn pax_tar(records: Vec<u8>, header_mtime: u64) -> Vec<u8> {
    let mut builder = Builder::new(Vec::new());
    let mut pax = Header::new_ustar();
    pax.set_entry_type(EntryType::XHeader);
    pax.set_path("PaxHeader/f").expect("pax path");
    pax.set_mode(0o644);
    pax.set_size(records.len() as u64);
    pax.set_cksum();
    builder.append(&pax, &records[..]).await.expect("pax");
    let mut file = Header::new_gnu();
    file.set_entry_type(EntryType::Regular);
    file.set_mode(0o644);
    file.set_uid(0);
    file.set_gid(0);
    file.set_mtime(header_mtime);
    file.set_size(2);
    builder
        .append_data(&mut file, "f", &b"hi"[..])
        .await
        .expect("file");
    builder.into_inner().await.expect("finish")
}

/// A writer over a vector, compressed with zstd when `zstd`
pub fn writer(zstd: bool) -> etar::Writer<Vec<u8>> {
    match zstd {
        true => etar::Writer::tar_zst(Vec::new()),
        false => etar::Writer::tar(Vec::new()),
    }
}

/// Read every header and body to the verified end, returning the entry count
pub async fn drain(bytes: Vec<u8>, limits: etar::ReadLimits) -> Result<u64, etar::Error> {
    let mut reader = etar::Reader::open(std::io::Cursor::new(bytes), limits).await?;
    let mut entries = 0;
    while let Some(mut entry) = reader.next_entry().await? {
        tokio::io::copy(&mut entry, &mut tokio::io::sink()).await?;
        entries += 1;
    }
    Ok(entries)
}
