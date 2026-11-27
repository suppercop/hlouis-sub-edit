use criterion::{Criterion, black_box, criterion_group, criterion_main};
use frame_cache::FrameCache;
use std::sync::Arc;

fn bench_frame_cache(c: &mut Criterion) {
    let frame_bytes = 4096;

    c.bench_function("frame_cache_get_hit", |b| {
        let mut cache = FrameCache::new(64 * frame_bytes, frame_bytes);
        for f in 0..64u32 {
            let mut buf = cache.acquire_buffer(frame_bytes);
            if let Some(inner) = Arc::get_mut(&mut buf) {
                inner.frame_number = f;
            }
            cache.insert(buf);
        }
        let mut f = 0u32;
        b.iter(|| {
            f = (f + 1) & 63;
            let got = cache.get(black_box(f));
            black_box(got);
        });
    });

    c.bench_function("frame_cache_steady_insert_with_pool", |b| {
        let mut cache = FrameCache::new(32 * frame_bytes, frame_bytes);
        for f in 0..34u32 {
            let mut buf = cache.acquire_buffer(frame_bytes);
            if let Some(inner) = Arc::get_mut(&mut buf) {
                inner.frame_number = f;
            }
            cache.insert(buf);
        }
        let mut f = 34u32;
        b.iter(|| {
            f = f.wrapping_add(1);
            let mut buf = cache.acquire_buffer(frame_bytes);
            if let Some(inner) = Arc::get_mut(&mut buf) {
                inner.frame_number = f;
            }
            let ok = cache.insert(black_box(buf));
            black_box(ok);
        });
    });
}

criterion_group!(benches, bench_frame_cache);
criterion_main!(benches);
