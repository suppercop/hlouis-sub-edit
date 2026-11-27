//! Plays 1 s of a generated tone on the default output device and reports latency and CPU.
//! Skips (with a message on stderr) when the machine has no output device, e.g. CI runners.

use audio_cache::{AudioCache, AudioCacheConfig};
use audio_out::{cycles_per_second, process_cpu_cycles, AudioOutError, AudioOutput, PcmSource};
use std::sync::Arc;
use std::time::{Duration, Instant};

const RATE: u32 = 44_100;

fn tone(seconds: u32, freq: f64) -> Arc<AudioCache<i16>> {
    let frames = u64::from(RATE * seconds);
    let cache = AudioCache::<i16>::new(AudioCacheConfig::new(RATE, 2, frames, 64 << 20)).unwrap();
    let bf = cache.config().block_frames;
    let total = usize::try_from(frames).unwrap();
    for b in 0..total.div_ceil(bf) {
        let n = bf.min(total - b * bf);
        let mut data = Vec::with_capacity(n * 2);
        for i in 0..n {
            let t = (b * bf + i) as f64 / f64::from(RATE);
            #[allow(clippy::cast_possible_truncation)]
            let v = (6_000.0 * (2.0 * std::f64::consts::PI * freq * t).sin()) as i16;
            data.extend_from_slice(&[v, v]);
        }
        cache.put_block(b, data.into_boxed_slice()).unwrap();
    }
    Arc::new(cache)
}

#[test]
fn plays_one_second_tone_and_reports_latency_and_cpu() {
    let source = tone(2, 440.0);
    let out = match AudioOutput::open_default(Arc::clone(&source) as Arc<dyn PcmSource>) {
        Ok(o) => o,
        Err(AudioOutError::NoDevice) => {
            eprintln!(
                "SKIPPED audio-out tone test: no audio output device on this machine \
                 (normal on headless CI runners)"
            );
            return;
        }
        Err(e) => {
            eprintln!("SKIPPED audio-out tone test: output device unusable: {e}");
            return;
        }
    };
    let info = out.info().clone();
    eprintln!("audio-out device: {info:?}");
    out.set_volume(0.25);

    // Let the stream settle, then reset counters by sampling them.
    std::thread::sleep(Duration::from_millis(100));
    let before = out.stats();
    let hz = cycles_per_second(Duration::from_millis(50));
    let cpu0 = process_cpu_cycles();
    let t0 = Instant::now();

    out.play(0, u64::from(RATE)); // exactly 1 s
    assert!(out.is_playing());
    let mut last = 0u64;
    let mut monotonic = true;
    while out.is_playing() && t0.elapsed() < Duration::from_secs(3) {
        let p = out.position();
        monotonic &= p >= last;
        last = p;
        std::thread::sleep(Duration::from_millis(16)); // UI-frame-rate polling
    }
    let wall = t0.elapsed();
    let cpu1 = process_cpu_cycles();
    let after = out.stats();

    assert!(!out.is_playing(), "playback did not finish in 3 s");
    assert!(monotonic, "playback clock went backwards");
    assert!(
        out.position() >= u64::from(RATE) - u64::from(RATE / 100),
        "clock ended at {} of {RATE}",
        out.position()
    );
    // Latency-compensated clock: the range is heard for ~1 s plus the output latency.
    let latency = after.latency;
    assert!(
        wall >= Duration::from_millis(950),
        "finished too early: {wall:?}"
    );

    let busy = after.busy.saturating_sub(before.busy);
    let cb_load = busy.as_secs_f64() / wall.as_secs_f64() * 100.0;
    // Includes this test's own 16 ms polling loop.
    let proc_cpu = match (hz, cpu0, cpu1) {
        (Some(hz), Some(a), Some(b)) => Some((b - a) as f64 / hz / wall.as_secs_f64() * 100.0),
        _ => None,
    };
    eprintln!(
        "audio-out: latency {:.1} ms, period {} frames, callbacks {}, max callback {:.0} us, \
         callback CPU {cb_load:.3} % of one core, process CPU {} % (wall {wall:?})",
        latency.as_secs_f64() * 1e3,
        after.frames_per_callback(),
        after.callbacks - before.callbacks,
        after.max_callback.as_secs_f64() * 1e6,
        proc_cpu.map_or_else(|| "n/a".to_owned(), |p| format!("{p:.2}")),
    );
    assert!(latency < Duration::from_millis(200), "latency {latency:?}");
    #[cfg(not(debug_assertions))]
    assert!(cb_load < 1.0, "callback CPU {cb_load:.3} %");

    out.play(u64::from(RATE / 2), u64::from(RATE));
    std::thread::sleep(Duration::from_millis(100));
    out.stop();
    let frozen = out.position();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(out.position(), frozen);
    assert!(!out.is_playing());
}
