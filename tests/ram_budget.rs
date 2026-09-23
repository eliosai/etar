//! Heap gate: pack and open peak memory stay bounded regardless of body size

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use etar::{EntryPath, Format, Kind, Meta, ReadLimits, Reader, WriteEntry, Writer};
use std::io::Cursor;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncReadExt, AsyncWrite};

/// Per-stream heap ceiling a single body must never breach
const BUDGET: usize = 16 * 1024 * 1024;
/// A body far larger than the budget, proving independence from body size
const HUGE: u64 = 256 * 1024 * 1024;

fn huge_entry() -> WriteEntry<tokio::io::Take<tokio::io::Repeat>> {
    WriteEntry::file(
        EntryPath::new("big.bin").unwrap(),
        Meta::new(0o644, 0, 0, 0, Kind::File),
        HUGE,
        tokio::io::repeat(0u8).take(HUGE),
    )
    .unwrap()
}

#[tokio::test]
async fn test_pack_and_open_heap_stay_under_budget() {
    let pack_peak = {
        let profiler = dhat::Profiler::builder().testing().build();
        let mut writer = Writer::new(tokio::io::sink(), Format::Tar);
        writer.append(huge_entry()).await.unwrap();
        writer.finish().await.unwrap();
        let peak = dhat::HeapStats::get().max_bytes;
        drop(profiler);
        peak
    };
    assert!(pack_peak < BUDGET, "pack peak heap {pack_peak} >= {BUDGET}");

    let tar = {
        let mut writer = Writer::new(Vec::new(), Format::Tar);
        writer.append(huge_entry()).await.unwrap();
        writer.finish().await.unwrap().sink
    };

    let open_peak = {
        let profiler = dhat::Profiler::builder().testing().build();
        let mut reader = Reader::open(
            Cursor::new(tar),
            ReadLimits::default().with_max_total_bytes(HUGE * 2),
        )
        .await
        .unwrap();
        while let Some(mut entry) = reader.next_entry().await.unwrap() {
            let _ = tokio::io::copy(&mut entry, &mut tokio::io::sink()).await;
        }
        let peak = dhat::HeapStats::get().max_bytes;
        drop(profiler);
        peak
    };
    assert!(open_peak < BUDGET, "open peak heap {open_peak} >= {BUDGET}");

    let slow_peak = {
        let profiler = dhat::Profiler::builder().testing().build();
        let mut writer = Writer::new(ThrottledSink::default(), Format::Tar);
        writer.append(huge_entry()).await.unwrap();
        writer.finish().await.unwrap();
        let peak = dhat::HeapStats::get().max_bytes;
        drop(profiler);
        peak
    };
    assert!(
        slow_peak < BUDGET,
        "pack-under-backpressure peak heap {slow_peak} >= {BUDGET}"
    );
}

/// A sink that pends every fourth poll and accepts small writes, forcing backpressure
#[derive(Default)]
struct ThrottledSink {
    polls: u32,
}

impl AsyncWrite for ThrottledSink {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let sink = self.get_mut();
        sink.polls = sink.polls.wrapping_add(1);
        if sink.polls.is_multiple_of(4) {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        Poll::Ready(Ok(buf.len().min(64 * 1024)))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
