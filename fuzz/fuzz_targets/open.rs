//! Fuzz the untrusted tar parser: arbitrary bytes must never panic, hang, or escape

use etar::{ReadLimits, Reader};
use std::io::Cursor;

fn main() {
    afl::fuzz!(|data: &[u8]| {
        let limits = ReadLimits {
            max_total_bytes: 8 << 20,
            max_entries: 10_000,
            max_entry_bytes: 4 << 20,
            ..ReadLimits::default()
        };
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().build() else {
            return;
        };
        runtime.block_on(drain(data.to_vec(), limits));
    });
}

/// Read every header and body until the first failure or the verified end
async fn drain(data: Vec<u8>, limits: ReadLimits) {
    let Ok(mut reader) = Reader::open(Cursor::new(data), limits).await else {
        return;
    };
    while let Ok(Some(mut entry)) = reader.next_entry().await {
        if tokio::io::copy(&mut entry, &mut tokio::io::sink())
            .await
            .is_err()
        {
            return;
        }
    }
}
