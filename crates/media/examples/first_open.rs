//! First-open experience on one file: index-free first frame, `FFMS2` first frame, full
//! `BestSource` index time (threads = cores-2 vs automatic, with CPU %), cached exact open.
//!
//! ```text
//! cargo run --release -p media --example first_open -- VIDEO [--skip-auto]
//! ```

use media::{
    background_decode_threads, process_cpu_time, set_current_thread_priority, BestSourceOptions,
    BestSourceProvider, FastOpen, FfmsVideo, HybridOptions, HybridVideo, IndexCache, IndexStore,
    QuickVideo, ThreadPriority, VideoProvider,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn index_run(path: &Path, threads: i32, cache: IndexCache) -> (f64, f64) {
    let path = path.to_path_buf();
    std::thread::spawn(move || {
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
    .expect("index thread")
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
    let path = file.expect("usage: first_open VIDEO");
    BestSourceProvider::set_ffmpeg_log_level(16);
    let cancel = AtomicBool::new(false);
    let mut buf = Vec::new();
    println!("file: {}", path.display());

    let t = Instant::now();
    let mut q = QuickVideo::open(&path, -1, 0).expect("quick open");
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
    println!(
        "quick (no index): first frame {quick_first:.0} ms, approx seek to {mid} {quick_seek:.0} ms, playback {quick_fps:.0} fps, est. {} frames",
        q.info().num_frames
    );
    drop(q);

    let t = Instant::now();
    let mut ff = FfmsVideo::open_with_progress(&path, &cancel, |_, _| {}).expect("ffms2");
    VideoProvider::decode_frame_into(&mut ff, 0, &mut buf).expect("frame 0");
    println!(
        "ffms2: open (index) + first frame {:.0} ms, {} frames",
        ms(t.elapsed()),
        ff.info().num_frames
    );
    drop(ff);

    let store_dir = std::env::temp_dir().join("hlouis-first-open-store");
    let _ = std::fs::remove_dir_all(&store_dir);
    let store = IndexStore::new(store_dir.clone(), IndexStore::DEFAULT_MAX_BYTES);
    let bg = background_decode_threads();
    let (wall, cpu) = index_run(&path, bg, store.cache_for(&path).expect("store"));
    println!("bestsource index, {bg} threads (cores-2), below-normal: {wall:.1} s, CPU {cpu:.0}%");
    if !skip_auto {
        let (wall, cpu) = index_run(&path, 0, IndexCache::Disabled);
        println!("bestsource index, auto threads: {wall:.1} s, CPU {cpu:.0}%");
    }

    let opts = HybridOptions {
        fast: FastOpen::Quick,
        bestsource: BestSourceOptions::default(),
        store: Some(store),
        priority: ThreadPriority::BelowNormal,
    };
    let t = Instant::now();
    let mut h = HybridVideo::open(&path, &opts, &cancel, |_, _| {}).expect("cached open");
    h.decode_frame_into(0, &mut buf).expect("frame 0");
    println!(
        "cached open -> first exact frame: {:.0} ms (provider {}, exact {})",
        ms(t.elapsed()),
        h.kind().as_str(),
        h.is_frame_exact()
    );
    drop(h);
    let _ = std::fs::remove_dir_all(&store_dir);
}
