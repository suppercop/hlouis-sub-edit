//! Criterion benchmark for 24-minute 48 kHz stereo waveform pyramid (< 150 ms, < 8 MB),
//! 10 s first-screen (< 5 ms), 2000-px column query (< 0.2 ms), and full 24-min STFT (< 1.2 s).

use audio_peaks::{PeakBucket, SpectrumBuilder, SpectrumConfig, WaveformPyramid};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

const SAMPLE_RATE: u32 = 48_000;
const EPISODE_FRAMES: usize = 24 * 60 * 48_000; // 69,120,000 stereo frames

fn generate_24min_stereo_i16() -> Vec<i16> {
    // Generate a 1-second tile (48k stereo frames) and repeat 1440x for fast bench setup.
    let tile_frames = 48_000usize;
    let mut tile = Vec::with_capacity(tile_frames * 2);
    for i in 0..tile_frames {
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let s = (((i.wrapping_mul(2_654_435_761)) >> 17) as i16) / 2;
        tile.push(s);
        tile.push(s.saturating_neg());
    }
    let mut full = Vec::with_capacity(EPISODE_FRAMES * 2);
    for _ in 0..(24 * 60) {
        full.extend_from_slice(&tile);
    }
    full
}

fn bench_audio_peaks(c: &mut Criterion) {
    let pcm_24m = generate_24min_stereo_i16();
    let first_screen_pcm = &pcm_24m[..10 * (SAMPLE_RATE as usize) * 2];

    let mut full_pyr = WaveformPyramid::new(SAMPLE_RATE, 2).expect("pyr");
    full_pyr.reserve_frames(EPISODE_FRAMES as u64);
    full_pyr.push_interleaved_i16(&pcm_24m);
    full_pyr.finish();
    #[allow(clippy::cast_precision_loss)]
    let mem_mb = (full_pyr.heap_bytes() as f64) / (1024.0 * 1024.0);
    println!("[audio-peaks] 24-min 48 kHz stereo pyramid heap: {mem_mb:.2} MiB (budget < 8 MB)");
    assert!(full_pyr.heap_bytes() < 8 * 1024 * 1024);

    c.bench_function("waveform_first_screen_10s", |b| {
        b.iter(|| {
            let mut pyr = WaveformPyramid::new(SAMPLE_RATE, 2).expect("pyr");
            pyr.push_interleaved_i16(black_box(first_screen_pcm));
            pyr.finish();
            black_box(pyr.total_frames())
        });
    });

    c.bench_function("waveform_query_2000px", |b| {
        let mut cols: Vec<PeakBucket> = Vec::with_capacity(2000);
        b.iter(|| {
            full_pyr.query_columns(0, EPISODE_FRAMES as u64, 2000, &mut cols);
            black_box(cols.len())
        });
    });

    let mut slow = c.benchmark_group("audio_peaks_24min");
    slow.sample_size(10);
    slow.bench_function("waveform_pyramid_24min_48khz_stereo", |b| {
        b.iter(|| {
            let mut pyr = WaveformPyramid::new(SAMPLE_RATE, 2).expect("pyr");
            pyr.reserve_frames(EPISODE_FRAMES as u64);
            pyr.push_interleaved_i16(black_box(&pcm_24m));
            pyr.finish();
            black_box(pyr.total_frames())
        });
    });

    slow.bench_function("spectrum_fft1024_50pct_24min_48khz", |b| {
        b.iter(|| {
            let mut spec = SpectrumBuilder::new(SpectrumConfig::new(SAMPLE_RATE, 2)).expect("spec");
            spec.push_interleaved_i16(black_box(&pcm_24m));
            black_box(spec.num_columns())
        });
    });
    slow.finish();
}

criterion_group!(benches, bench_audio_peaks);
criterion_main!(benches);
