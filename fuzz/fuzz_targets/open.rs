//! Fuzz the untrusted tar parser: arbitrary bytes must never panic, hang, or escape

use etar::{ReadLimits, validate};
use std::io::Cursor;

fn main() {
    afl::fuzz!(|data: &[u8]| {
        let limits = ReadLimits::default()
            .with_max_total_bytes(8 << 20)
            .with_max_entries(10_000)
            .with_max_entry_bytes(4 << 20);
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().build() else {
            return;
        };
        runtime.block_on(async {
            let _ = validate(Cursor::new(data.to_vec()), limits).await;
        });
    });
}
