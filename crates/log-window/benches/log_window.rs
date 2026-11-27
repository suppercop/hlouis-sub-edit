use criterion::{Criterion, black_box, criterion_group, criterion_main};
use log_window::{LogLevel, LogRecord, RingLogBuffer};

fn bench_log_window(c: &mut Criterion) {
    let buf = RingLogBuffer::new(4096);
    c.bench_function("log_window_ring_push", |b| {
        let mut i = 0u64;
        b.iter(|| {
            i += 1;
            buf.push(LogRecord {
                timestamp_ms: i,
                level: LogLevel::Info,
                target: String::new(),
                message: String::new(),
            });
            black_box(buf.len());
        });
    });
}

criterion_group!(benches, bench_log_window);
criterion_main!(benches);
