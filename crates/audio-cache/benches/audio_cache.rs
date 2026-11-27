use audio_cache::{AudioCache, AudioCacheConfig, DEFAULT_BLOCK_FRAMES};
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

fn bench_audio_cache(c: &mut Criterion) {
    // 60 seconds of 48 kHz stereo i16 (~11.5 MB) hot in RAM for random 100 ms read bench.
    let total_frames = 60 * 48_000u64;
    let cfg = AudioCacheConfig::new(48_000, 2, total_frames, 64 * 1024 * 1024);
    let cache = AudioCache::<i16>::new(cfg).expect("cache");
    let num_blocks =
        usize::try_from(total_frames.div_ceil(DEFAULT_BLOCK_FRAMES as u64)).expect("nb");
    for b in 0..num_blocks {
        cache
            .put_block(b, vec![42i16; DEFAULT_BLOCK_FRAMES * 2].into_boxed_slice())
            .expect("put");
    }

    // 100 ms at 48 kHz stereo = 4,800 frames = 9,600 i16 samples
    let mut read_buf = vec![0i16; 4_800 * 2];
    let mut pos = 10_000u64;
    c.bench_function("audio_cache_random_100ms_hot_read", |b| {
        b.iter(|| {
            pos = (pos + 97_331) % (total_frames - 4_800);
            let n = cache
                .read_frames(black_box(pos), black_box(&mut read_buf))
                .expect("read");
            black_box(n);
        });
    });

    // Memory fill throughput: 64 blocks = 16 MiB
    let fill_blocks = 64usize;
    let fill_bytes = (fill_blocks * DEFAULT_BLOCK_FRAMES * 2 * 2) as u64;
    let src_block = vec![123i16; DEFAULT_BLOCK_FRAMES * 2];
    let mut group = c.benchmark_group("audio_cache_fill");
    group.throughput(Throughput::Bytes(fill_bytes));
    group.bench_function("fill_16mb_from_memory", |b| {
        let fill_cfg = AudioCacheConfig::new(
            48_000,
            2,
            (fill_blocks * DEFAULT_BLOCK_FRAMES) as u64,
            64 * 1024 * 1024,
        );
        let fill_cache = AudioCache::<i16>::new(fill_cfg).expect("fc");
        b.iter(|| {
            for idx in 0..fill_blocks {
                fill_cache
                    .put_block(idx, src_block.clone().into_boxed_slice())
                    .expect("put");
            }
            black_box(fill_cache.filled_frames());
        });
    });
    group.finish();
}

criterion_group!(benches, bench_audio_cache);
criterion_main!(benches);
