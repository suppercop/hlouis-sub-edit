//! Render cost of one second of output (100 WASAPI-sized 480-frame buffers), with and without
//! 44.1 -> 48 kHz conversion.

use audio_cache::{AudioCache, AudioCacheConfig};
use audio_out::{OfflineRenderer, PcmSource};
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;
use std::sync::Arc;

fn source(rate: u32) -> Arc<dyn PcmSource> {
    let frames = u64::from(rate) * 4;
    let cache =
        AudioCache::<i16>::new(AudioCacheConfig::new(rate, 2, frames, 64 << 20)).expect("cache");
    let bf = cache.config().block_frames;
    let total = usize::try_from(frames).expect("frames");
    for b in 0..total.div_ceil(bf) {
        let n = bf.min(total - b * bf);
        let block: Vec<i16> = (0..n * 2)
            .map(|i| i16::try_from((i * 37) % 20_000).unwrap_or(0) - 10_000)
            .collect();
        cache.put_block(b, block.into_boxed_slice()).expect("put");
    }
    Arc::new(cache)
}

fn bench(c: &mut Criterion) {
    for (name, rate) in [
        ("resample_44k1_to_48k", 44_100u32),
        ("passthrough_48k", 48_000),
    ] {
        let mut r = OfflineRenderer::new(source(rate), 48_000, 2).expect("renderer");
        let mut buf = vec![0.0f32; 480 * 2];
        c.bench_function(&format!("audio_out/render_1s_{name}"), |b| {
            b.iter(|| {
                r.play(0, u64::from(rate) * 2);
                for _ in 0..100 {
                    r.render(&mut buf);
                }
                black_box(buf[0])
            });
        });
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
