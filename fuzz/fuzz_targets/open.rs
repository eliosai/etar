//! Fuzz the untrusted tar parser: arbitrary bytes must never panic, hang, or escape

use futures::StreamExt;
use std::io::Cursor;
use tara::SecurityLimits;

fn main() {
    afl::fuzz!(|data: &[u8]| {
        let limits = SecurityLimits::default()
            .with_max_total_bytes(8 << 20)
            .with_max_entries(10_000)
            .with_max_entry_bytes(4 << 20);
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().build() else {
            return;
        };
        runtime.block_on(async {
            let stream = tara::open(Cursor::new(data.to_vec()), limits);
            futures::pin_mut!(stream);
            while let Some(Ok(mut entry)) = stream.next().await {
                let _ = tokio::io::copy(&mut entry, &mut tokio::io::sink()).await;
            }
        });
    });
}
