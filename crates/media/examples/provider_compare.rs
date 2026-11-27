//! `BestSource` vs `FFMS2` on the same file: open, random seek latency, sequential decode fps.
//!
//! ```text
//! cargo run --release -p media --example provider_compare -- [VIDEO] [--seeks 200] [--seq 1000]
//! ```
//! Without `VIDEO`, a 2-minute 1080p H.264 clip is generated with the project's LGPL
//! `ffmpeg.exe` (`.native/tools`, or `HLOUIS_FFMPEG`) into the temp directory.

use media::{
    BestSourceOptions, BestSourceProvider, FfmsVideo, IndexCache, ProviderKind, VideoProvider,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn generate_clip() -> PathBuf {
    let ffmpeg = std::env::var("HLOUIS_FFMPEG").map_or_else(
        |_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.native/tools/ffmpeg.exe"),
        PathBuf::from,
    );
    let out = std::env::temp_dir().join("hlouis-bench-1080p-h264.mkv");
    if out.exists() {
        return out;
    }
    eprintln!("generating {} ...", out.display());
    let status = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .args(["-i", "testsrc2=size=1920x1080:rate=24000/1001:duration=120"])
        .args(["-c:v", "libopenh264", "-b:v", "6M", "-g", "240"])
        .arg(&out)
        .status()
        .expect("spawn ffmpeg");
    assert!(status.success(), "ffmpeg failed");
    out
}

struct Stats {
    open_ms: f64,
    first_frame_ms: f64,
    p50: f64,
    p95: f64,
    worst: f64,
    seq_fps: f64,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let idx = ((sorted.len() as f64 - 1.0) * p).round();
    // idx is within 0..len by construction.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    sorted[idx as usize]
}

fn seek_targets(total: usize, n: usize) -> Vec<usize> {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    (0..n)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            usize::try_from(state % total as u64).expect("fits")
        })
        .collect()
}

fn measure(
    v: &mut dyn VideoProvider,
    open_ms: f64,
    first_frame_ms: f64,
    seeks: usize,
    seq: usize,
) -> Stats {
    let total = v.info().num_frames;
    let mut buf = Vec::new();
    let mut lat: Vec<f64> = seek_targets(total, seeks)
        .into_iter()
        .map(|n| {
            let t = Instant::now();
            v.decode_frame_into(n, &mut buf).expect("seek decode");
            ms(t.elapsed())
        })
        .collect();
    lat.sort_by(f64::total_cmp);
    let start = total / 3;
    let end = (start + seq).min(total);
    v.decode_frame_into(start, &mut buf).expect("seq start");
    let t = Instant::now();
    for n in start + 1..end {
        v.decode_frame_into(n, &mut buf).expect("seq decode");
    }
    let seq_fps = (end - start - 1) as f64 / t.elapsed().as_secs_f64();
    Stats {
        open_ms,
        first_frame_ms,
        p50: percentile(&lat, 0.5),
        p95: percentile(&lat, 0.95),
        worst: *lat.last().unwrap_or(&0.0),
        seq_fps,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut file = None;
    let mut seeks = 200usize;
    let mut seq = 1000usize;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--seeks" => seeks = args.next().and_then(|s| s.parse().ok()).unwrap_or(seeks),
            "--seq" => seq = args.next().and_then(|s| s.parse().ok()).unwrap_or(seq),
            _ => file = Some(PathBuf::from(a)),
        }
    }
    let path = file.unwrap_or_else(generate_clip);
    let cancel = AtomicBool::new(false);
    BestSourceProvider::set_ffmpeg_log_level(16);
    println!("file: {}", path.display());

    // BestSource: cold index into a private cache dir, then a cached open.
    let cache_dir = std::env::temp_dir().join("hlouis-bench-bsindex");
    let _ = std::fs::remove_dir_all(&cache_dir);
    let opts = BestSourceOptions {
        cache: IndexCache::Dir(cache_dir.clone()),
        ..BestSourceOptions::default()
    };
    let t = Instant::now();
    let cold = BestSourceProvider::open_with_progress(&path, &opts, &cancel, |_, _| {})
        .expect("BestSource index");
    let bs_index_ms = ms(t.elapsed());
    drop(cold);
    let t = Instant::now();
    let mut bs = BestSourceProvider::open_with_progress(&path, &opts, &cancel, |_, _| {})
        .expect("BestSource cached open");
    let bs_open_ms = ms(t.elapsed());
    let mut buf = Vec::new();
    bs.decode_frame_into(0, &mut buf).expect("frame 0");
    let bs_first = ms(t.elapsed());
    let info = bs.info().clone();
    println!(
        "{}x{} {} frames {:.3} fps, {} keyframes",
        info.width,
        info.height,
        info.num_frames,
        info.fps,
        info.keyframes.len()
    );
    let bs_stats = measure(&mut bs, bs_open_ms, bs_first, seeks, seq);
    drop(bs);

    // FFMS2 (this crate has no FFMS2 on-disk index yet, so every open indexes).
    let t = Instant::now();
    let mut ff = FfmsVideo::open_with_progress(&path, &cancel, |_, _| {}).expect("FFMS2 open");
    let ff_open_ms = ms(t.elapsed());
    VideoProvider::decode_frame_into(&mut ff, 0, &mut buf).expect("frame 0");
    let ff_first = ms(t.elapsed());
    if ff.info().num_frames != info.num_frames || ff.info().keyframes != info.keyframes {
        println!(
            "note: FFMS2 sees {} frames / {} keyframes",
            ff.info().num_frames,
            ff.info().keyframes.len()
        );
    }
    let ff_stats = measure(&mut ff, ff_open_ms, ff_first, seeks, seq);

    println!("BestSource cold index: {bs_index_ms:.0} ms");
    println!(
        "| provider | open (ms) | open->first frame (ms) | seek p50 | seek p95 | seek worst | seq fps |"
    );
    println!("|---|---|---|---|---|---|---|");
    for (kind, s) in [
        (ProviderKind::BestSource, &bs_stats),
        (ProviderKind::Ffms2, &ff_stats),
    ] {
        println!(
            "| {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} |",
            kind.as_str(),
            s.open_ms,
            s.first_frame_ms,
            s.p50,
            s.p95,
            s.worst,
            s.seq_fps
        );
    }
    let _ = std::fs::remove_dir_all(&cache_dir);
}
