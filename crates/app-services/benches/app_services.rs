use app_services::{LogLevel, LogRingBuffer};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_app_services(c: &mut Criterion) {
    // Warm up a 4096-entry ring buffer to measure steady-state overwrite append (< 100 ns).
    let mut ring = LogRingBuffer::with_capacity(4096);
    for i in 0..4096u64 {
        ring.push(
            i,
            LogLevel::Info,
            "media",
            "decoded video frame successfully",
        );
    }

    c.bench_function("app_services_log_append_steady_state", |b| {
        let mut ts = 5000u64;
        b.iter(|| {
            ts += 1;
            ring.push(
                black_box(ts),
                LogLevel::Info,
                "media",
                black_box("decoded video frame successfully"),
            );
        });
    });

    // 100,000 log entries for filter + case-insensitive search bench (< 2 ms).
    let mut big = LogRingBuffer::with_capacity(100_000);
    for i in 0..100_000u64 {
        let (lvl, msg) = if i % 50 == 0 {
            (
                LogLevel::Warn,
                "libass Font fallback triggered for CJK glyph",
            )
        } else {
            (LogLevel::Info, "routine playback tick completed ok")
        };
        big.push(i, lvl, "render", msg);
    }
    let mut hits = Vec::with_capacity(2048);

    c.bench_function("app_services_filter_100k_entries", |b| {
        b.iter(|| {
            big.filter_into(LogLevel::Warn, None, Some("fallback"), &mut hits);
            black_box(hits.len());
        });
    });
}

criterion_group!(benches, bench_app_services);
criterion_main!(benches);
