//! Plays a 440 Hz tone for `N` seconds (default 3) and prints latency / CPU figures.
//!
//! `cargo run --release -p audio-out --example play_tone -- 3`

use audio_cache::{AudioCache, AudioCacheConfig};
use audio_out::{cycles_per_second, process_cpu_cycles, process_cpu_time, AudioOutput, PcmSource};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let secs: u32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let rate = 44_100u32;
    let frames = u64::from(rate * secs);
    let cache = AudioCache::<f32>::new(AudioCacheConfig::new(rate, 1, frames, 256 << 20))?;
    let bf = cache.config().block_frames;
    let total = usize::try_from(frames)?;
    for b in 0..total.div_ceil(bf) {
        let n = bf.min(total - b * bf);
        let block: Vec<f32> = (0..n)
            .map(|i| {
                let t = (b * bf + i) as f32 / rate as f32;
                0.2 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            })
            .collect();
        cache.put_block(b, block.into_boxed_slice())?;
    }
    let out = AudioOutput::open_default(Arc::new(cache) as Arc<dyn PcmSource>)?;
    println!("{:?}", out.info());
    std::thread::sleep(Duration::from_millis(200));
    let s0 = out.stats();
    let hz = cycles_per_second(Duration::from_millis(100));
    let c0 = process_cpu_time();
    let k0 = process_cpu_cycles();
    let t0 = Instant::now();
    out.play(0, frames);
    while out.is_playing() {
        std::thread::sleep(Duration::from_millis(250));
        println!(
            "position {:>8} / {frames}  latency {:.1} ms",
            out.position(),
            out.latency().as_secs_f64() * 1e3
        );
    }
    let wall = t0.elapsed().as_secs_f64();
    let s1 = out.stats();
    let busy = s1.busy.saturating_sub(s0.busy).as_secs_f64();
    println!(
        "period {} frames, max callback {:.0} us, callback CPU {:.3} % of one core",
        s1.frames_per_callback(),
        s1.max_callback.as_secs_f64() * 1e6,
        busy / wall * 100.0
    );
    if let (Some(a), Some(b)) = (c0, process_cpu_time()) {
        println!(
            "process CPU while playing {:.2} % of one core",
            b.saturating_sub(a).as_secs_f64() / wall * 100.0
        );
    }
    if let (Some(hz), Some(a), Some(b)) = (hz, k0, process_cpu_cycles()) {
        println!(
            "process CPU while playing (cycle-accurate) {:.3} % of one core",
            (b - a) as f64 / hz / wall * 100.0
        );
    }
    Ok(())
}
