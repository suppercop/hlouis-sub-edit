//! First-open experience on one file: index-free first frame, approximate seek, progressive
//! exact seek, full `BestSource` video + audio index time (threads = cores-2, below-normal),
//! peak private memory during indexing, cached exact open, and `FFMS2` comparison.
//!
//! ```text
//! cargo run --release -p media --example first_open -- VIDEO [--skip-auto]
//! ```

use media::{
    background_decode_threads, process_cpu_time, process_memory_bytes, set_current_thread_priority,
    AudioUpgradeStatus, BestSourceAudio, BestSourceOptions, BestSourceProvider, FastOpen,
    FfmsVideo, HybridOptions, HybridVideo, IndexCache, IndexStore, QuickVideo, ThreadPriority,
    VideoProvider,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn spawn_mem_sampler(stop: Arc<AtomicBool>) -> std::thread::JoinHandle<(u64, u64)> {
    std::thread::spawn(move || {
        let mut peak_priv = 0u64;
        let mut peak_commit = 0u64;
        while !stop.load(Ordering::Relaxed) {
            if let Some((priv_b, commit_b)) = process_memory_bytes() {
                peak_priv = peak_priv.max(priv_b);
                peak_commit = peak_commit.max(commit_b);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if let Some((priv_b, commit_b)) = process_memory_bytes() {
            peak_priv = peak_priv.max(priv_b);
            peak_commit = peak_commit.max(commit_b);
        }
        (peak_priv, peak_commit)
    })
}

fn measure_quick(path: &Path) -> usize {
    let mut buf = Vec::new();
    let t = Instant::now();
    let mut q = QuickVideo::open(path, -1, 4).expect("quick open");
    q.decode_frame_into(0, &mut buf).expect("frame 0");
    let quick_first = ms(t.elapsed());
    let mid = q.info().num_frames / 2;
    let t = Instant::now();
    q.decode_frame_into(mid, &mut buf).expect("mid");
    let quick_seek = ms(t.elapsed());
    let t = Instant::now();
    for n in mid + 1..mid + 49 {
        q.decode_frame_into(n, &mut buf).expect("seq");
    }
    let quick_fps = 48.0 / t.elapsed().as_secs_f64();
    let est_frames = q.info().num_frames;
    println!(
        "quick (no index): first frame {quick_first:.0} ms, approx seek to {mid} {quick_seek:.0} ms, playback {quick_fps:.0} fps, est. {est_frames} frames"
    );
    mid
}

fn measure_ffms2(path: &Path, mid: usize) {
    let cancel = AtomicBool::new(false);
    let mut buf = Vec::new();
    let ff_base_priv = process_memory_bytes().map_or(0, |m| m.0);
    let ff_stop = Arc::new(AtomicBool::new(false));
    let ff_sampler = spawn_mem_sampler(Arc::clone(&ff_stop));
    let t = Instant::now();
    let mut ff = FfmsVideo::open_with_progress(path, &cancel, |_, _| {}).expect("ffms2");
    VideoProvider::decode_frame_into(&mut ff, 0, &mut buf).expect("frame 0");
    let ff_first = ms(t.elapsed());
    let t = Instant::now();
    VideoProvider::decode_frame_into(&mut ff, mid, &mut buf).expect("ffms2 mid seek");
    let ff_seek = ms(t.elapsed());
    let ff_frames = ff.info().num_frames;
    drop(ff);
    ff_stop.store(true, Ordering::Relaxed);
    let (ff_peak_priv, _) = ff_sampler.join().expect("ff sampler");
    let ff_delta_priv = ff_peak_priv.saturating_sub(ff_base_priv);
    println!(
        "ffms2: open (index) + first frame {ff_first:.0} ms, seek to {mid} {ff_seek:.0} ms, {ff_frames} frames, peak private +{:.1} MiB (process {:.1} MiB)",
        mib(ff_delta_priv),
        mib(ff_peak_priv)
    );
}

fn index_run(path: &Path, threads: i32, cache: IndexCache) -> (f64, f64, u64, u64) {
    let path = path.to_path_buf();
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = spawn_mem_sampler(Arc::clone(&stop));
    let base_priv = process_memory_bytes().map_or(0, |m| m.0);
    let (wall_s, cpu_pct) = std::thread::spawn(move || {
        let _ = set_current_thread_priority(ThreadPriority::BelowNormal);
        let cancel = AtomicBool::new(false);
        let cpu0 = process_cpu_time();
        let t = Instant::now();
        let opts = BestSourceOptions {
            threads,
            cache,
            ..BestSourceOptions::default()
        };
        let v = BestSourceProvider::open_with_progress(&path, &opts, &cancel, |_, _| {})
            .expect("index");
        let wall = t.elapsed();
        let cpu = process_cpu_time().saturating_sub(cpu0);
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        drop(v);
        (
            wall.as_secs_f64(),
            100.0 * cpu.as_secs_f64() / wall.as_secs_f64() / cores as f64,
        )
    })
    .join()
    .expect("index thread");
    stop.store(true, Ordering::Relaxed);
    let (peak_priv, peak_commit) = sampler.join().expect("mem sampler");
    (
        wall_s,
        cpu_pct,
        peak_priv.saturating_sub(base_priv),
        peak_priv.max(peak_commit),
    )
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut file = None;
    let mut skip_auto = false;
    for a in args.by_ref() {
        if a == "--skip-auto" {
            skip_auto = true;
        } else {
            file = Some(PathBuf::from(a));
        }
    }
    let path = file.expect("usage: first_open VIDEO [--skip-auto]");
    BestSourceProvider::set_ffmpeg_log_level(16);
    println!("file: {}", path.display());

    let mid = measure_quick(&path);
    measure_ffms2(&path, mid);

    let store_dir = std::env::temp_dir().join("hlouis-first-open-store");
    let _ = std::fs::remove_dir_all(&store_dir);
    let store = IndexStore::new(store_dir.clone(), IndexStore::DEFAULT_MAX_BYTES);
    let bg = background_decode_threads();
    let (wall, cpu, delta_priv, peak_proc) =
        index_run(&path, bg, store.cache_for(&path).expect("store"));
    println!(
        "bestsource video index, {bg} threads (cores-2), below-normal: {wall:.1} s, CPU {cpu:.0}%, peak private +{:.1} MiB (process peak {:.1} MiB)",
        mib(delta_priv),
        mib(peak_proc)
    );

    let cancel = AtomicBool::new(false);
    let aud_opts = BestSourceOptions {
        track: -1,
        threads: bg.min(4),
        cache: store.cache_for(&path).expect("store"),
    };
    let t = Instant::now();
    let aud = BestSourceAudio::open_with_progress(&path, &aud_opts, &cancel, |_, _| {})
        .expect("audio index");
    println!("bestsource audio index: {:.2} s", t.elapsed().as_secs_f64());
    drop(aud);

    if !skip_auto {
        let (wall_a, cpu_a, delta_a, peak_a) = index_run(&path, 0, IndexCache::Disabled);
        println!(
            "bestsource index, auto threads: {wall_a:.1} s, CPU {cpu_a:.0}%, peak private +{:.1} MiB (process {:.1} MiB)",
            mib(delta_a),
            mib(peak_a)
        );
    }

    let opts = HybridOptions {
        fast: FastOpen::Quick,
        bestsource: BestSourceOptions::default(),
        audio_track: -1,
        index_audio: true,
        store: Some(store),
        priority: ThreadPriority::BelowNormal,
    };
    let mut buf = Vec::new();
    let t = Instant::now();
    let mut h = HybridVideo::open(&path, &opts, &cancel, |_, _| {}).expect("cached open");
    h.decode_frame_into(0, &mut buf).expect("frame 0");
    let cached_first = ms(t.elapsed());
    let t = Instant::now();
    h.decode_frame_into(mid, &mut buf).expect("cached mid");
    let cached_seek = ms(t.elapsed());
    println!(
        "cached reopen -> first exact frame: {cached_first:.0} ms, seek to {mid}: {cached_seek:.0} ms (provider {}, exact {}, audio {:?})",
        h.kind().as_str(),
        h.is_frame_exact(),
        match h.poll_audio() {
            AudioUpgradeStatus::Ready => "ready",
            AudioUpgradeStatus::NoAudio => "none",
            AudioUpgradeStatus::Pending => "pending",
            AudioUpgradeStatus::Failed(_) => "failed",
        }
    );
    drop(h);
    let _ = std::fs::remove_dir_all(&store_dir);
}
