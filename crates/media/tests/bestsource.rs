//! `BestSource` provider tests on clips generated at test time with the project's LGPL
//! `ffmpeg.exe` (`.native/tools`, or `HLOUIS_FFMPEG`). No media is committed.

use media::{
    AudioProvider, BestSourceAudio, BestSourceOptions, BestSourceProvider, FfmsVideo, HybridVideo,
    IndexCache, MediaError, PlanarFrame, ProviderKind, UpgradeStatus, VideoProvider,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

fn ffmpeg() -> Option<PathBuf> {
    tool("ffmpeg", "HLOUIS_FFMPEG")
}

fn tool(name: &str, env_var: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var(env_var) {
        return Some(PathBuf::from(p));
    }
    let native = std::env::var("HLOUIS_NATIVE_DIR").map_or_else(
        |_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.native"),
        PathBuf::from,
    );
    let exe = native.join("tools").join(format!("{name}.exe"));
    if exe.exists() {
        Some(exe)
    } else {
        assert!(
            std::env::var_os("CI").is_none(),
            "project {name}.exe not found in CI ({})",
            exe.display()
        );
        eprintln!("skipping: {} not found", exe.display());
        None
    }
}

fn work_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hlouis-bs-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Keyframes in display order, from the container's packet flags (ffprobe ground truth).
fn probe_keyframes(path: &Path) -> Vec<usize> {
    let exe = tool("ffprobe", "HLOUIS_FFPROBE").expect("ffprobe");
    let out = Command::new(exe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .args(["packet=pts,flags", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .expect("spawn ffprobe");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pkts: Vec<(i64, bool)> = text
        .lines()
        .filter_map(|l| {
            let (pts, flags) = l.split_once(',')?;
            Some((pts.trim().parse().ok()?, flags.starts_with('K')))
        })
        .collect();
    pkts.sort_unstable();
    pkts.iter()
        .enumerate()
        .filter_map(|(i, &(_, k))| k.then_some(i))
        .collect()
}

fn run_ffmpeg(args: &[&str]) {
    let exe = ffmpeg().expect("ffmpeg");
    let out = Command::new(exe)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .output()
        .expect("spawn ffmpeg");
    assert!(
        out.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

const CFR_FRAMES: usize = 240;
const GOP: usize = 24;
const AUDIO_SAMPLES: u64 = 48_000 * 5;

/// 320x240 MPEG-4 ASP with B-frames (reordering), fixed GOP 24, at 24000/1001,
/// plus 5 s of 48 kHz stereo FLAC.
fn cfr_clip() -> Option<&'static Path> {
    static CLIP: OnceLock<Option<PathBuf>> = OnceLock::new();
    CLIP.get_or_init(|| {
        ffmpeg()?;
        let out = work_dir().join("cfr.mkv");
        let frames = CFR_FRAMES.to_string();
        let gop = GOP.to_string();
        run_ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=24000/1001",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=5",
            "-frames:v",
            &frames,
            "-c:v",
            "mpeg4",
            "-q:v",
            "3",
            "-bf",
            "2",
            "-g",
            &gop,
            "-sc_threshold",
            "1000000000",
            "-ac",
            "2",
            "-c:a",
            "flac",
            out.to_str().expect("utf8 path"),
        ]);
        Some(out)
    })
    .as_deref()
}

const VFR_FRAMES: usize = 96;

fn vfr_expected_ms(i: usize) -> i64 {
    let i = i64::try_from(i).expect("small index");
    if i < 48 {
        i * 40
    } else {
        1920 + (i - 48) * 20
    }
}

/// 48 frames at 40 ms then 48 frames at 20 ms (true VFR in Matroska, 1 ms timebase).
fn vfr_clip() -> Option<&'static Path> {
    static CLIP: OnceLock<Option<PathBuf>> = OnceLock::new();
    CLIP.get_or_init(|| {
        ffmpeg()?;
        let out = work_dir().join("vfr.mkv");
        let frames = VFR_FRAMES.to_string();
        run_ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=50",
            "-vf",
            "settb=1/1000,setpts='if(lt(N,48),N*40,1920+(N-48)*20)'",
            "-fps_mode",
            "passthrough",
            "-enc_time_base",
            "1:1000",
            "-frames:v",
            &frames,
            "-c:v",
            "mpeg4",
            "-q:v",
            "3",
            "-g",
            "12",
            out.to_str().expect("utf8 path"),
        ]);
        Some(out)
    })
    .as_deref()
}

fn no_cache() -> BestSourceOptions {
    BestSourceOptions {
        cache: IndexCache::Disabled,
        ..BestSourceOptions::default()
    }
}

fn open_bs(path: &Path) -> BestSourceProvider {
    let cancel = AtomicBool::new(false);
    BestSourceProvider::open_with_progress(path, &no_cache(), &cancel, |_, _| {})
        .expect("open with BestSource")
}

fn checksum(data: &[u8]) -> u64 {
    // FNV-1a: stable, dependency-free.
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[test]
fn opens_generated_clip_with_expected_metadata() {
    let Some(clip) = cfr_clip() else { return };
    let v = open_bs(clip);
    let info = v.info();
    assert_eq!(v.kind(), ProviderKind::BestSource);
    assert_eq!((info.width, info.height), (320, 240));
    assert_eq!(info.num_frames, CFR_FRAMES);
    assert_eq!((info.fps_num, info.fps_den), (24000, 1001));
    assert!(
        (info.duration_ms - 10_010.0).abs() < 2.0,
        "{}",
        info.duration_ms
    );
    let planar = v.planar_format();
    assert_eq!(planar.color_family, 3);
    assert_eq!(planar.bits, 8);
    assert_eq!((planar.sub_sampling_w, planar.sub_sampling_h), (1, 1));
}

#[test]
fn random_seeks_match_sequential_decode() {
    let Some(clip) = cfr_clip() else { return };
    let mut v = open_bs(clip);
    let mut buf = Vec::new();
    let mut seq = Vec::with_capacity(CFR_FRAMES);
    for n in 0..CFR_FRAMES {
        let pts = v.decode_frame_into(n, &mut buf).expect("sequential decode");
        assert_eq!(pts, v.info().frame_pts_ms[n]);
        assert_eq!(buf.len(), 320 * 240 * 4);
        seq.push(checksum(&buf));
    }
    // testsrc2 has a frame counter, so every frame must be distinct.
    let mut uniq = seq.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(uniq.len(), CFR_FRAMES, "decoded frames are not distinct");

    let mut v2 = open_bs(clip);
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut targets = vec![CFR_FRAMES - 1, 0, GOP, GOP - 1, GOP + 1, 5 * GOP + 7];
    for _ in 0..80 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        targets.push(usize::try_from(state % CFR_FRAMES as u64).expect("fits"));
    }
    for n in targets {
        v2.decode_frame_into(n, &mut buf).expect("random decode");
        assert_eq!(checksum(&buf), seq[n], "frame {n} differs after seek");
    }
}

#[test]
fn ffms2_and_bestsource_agree_on_frames_keyframes_and_timestamps() {
    let Some(clip) = cfr_clip() else { return };
    let cancel = AtomicBool::new(false);
    let mut ff = FfmsVideo::open_with_progress(clip, &cancel, |_, _| {}).expect("ffms2 open");
    let mut bs = open_bs(clip);
    let truth = probe_keyframes(clip);
    // Fixed GOP; the encoder may also code the very last frame as an I-frame.
    let gop_kf: Vec<usize> = (0..CFR_FRAMES).step_by(GOP).collect();
    assert!(truth.starts_with(&gop_kf), "{truth:?}");
    assert_eq!(bs.info().keyframes, truth);
    // FFMS2 must agree on the regular GOP keyframes (it may drop a trailing one).
    assert!(ff.info().keyframes.iter().all(|k| truth.contains(k)));
    assert!(ff.info().keyframes.starts_with(&gop_kf));
    assert_eq!(bs.info().frame_pts_ms, ff.info().frame_pts_ms);
    assert_eq!(bs.next_keyframe(0), Some(GOP));
    assert_eq!(bs.prev_keyframe(GOP), Some(0));
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for n in [0, 1, 50, 239] {
        VideoProvider::decode_frame_into(&mut ff, n, &mut a).expect("ffms decode");
        bs.decode_frame_into(n, &mut b).expect("bs decode");
        assert_eq!(a.len(), b.len());
        // Same source pixels; tolerate small YUV->RGB rounding differences between paths.
        let diff: u64 = a
            .iter()
            .zip(&b)
            .map(|(&x, &y)| u64::from(x.abs_diff(y)))
            .sum();
        let mean = diff as f64 / a.len() as f64;
        assert!(mean < 3.0, "frame {n}: mean abs diff {mean}");
    }
}

#[test]
fn vfr_timecodes_are_exact() {
    let Some(clip) = vfr_clip() else { return };
    let v = open_bs(clip);
    let info = v.info();
    assert_eq!(info.num_frames, VFR_FRAMES);
    let first = info.frame_pts_ms[0];
    for (i, &pts) in info.frame_pts_ms.iter().enumerate() {
        assert_eq!(pts - first, vfr_expected_ms(i), "frame {i}");
    }
    assert_eq!(info.keyframes, probe_keyframes(clip));
    assert!(info.keyframes.starts_with(&[0, 12, 24, 36, 48]));
}

#[test]
fn planar_yuv_export() {
    let Some(clip) = cfr_clip() else { return };
    let mut v = open_bs(clip);
    let mut f = PlanarFrame::default();
    v.decode_planar_into(100, &mut f).expect("planar decode");
    assert_eq!(f.strides, [320, 160, 160]);
    assert_eq!(f.planes[0].len(), 320 * 240);
    assert_eq!(f.planes[1].len(), 160 * 120);
    let luma_sum: u64 = f.planes[0].iter().map(|&x| u64::from(x)).sum();
    assert!(luma_sum > 0);
    let mut g = PlanarFrame::default();
    v.decode_planar_into(3, &mut g).expect("planar decode");
    v.decode_planar_into(100, &mut g)
        .expect("planar decode again");
    assert_eq!(f.planes, g.planes);
}

#[test]
fn audio_sample_count_and_reads() {
    let Some(clip) = cfr_clip() else { return };
    let cancel = AtomicBool::new(false);
    let mut a = BestSourceAudio::open_with_progress(clip, &no_cache(), &cancel, |_, _| {})
        .expect("open audio");
    let info = a.audio_info().clone();
    assert_eq!(a.kind(), ProviderKind::BestSource);
    assert_eq!(info.sample_rate, 48_000);
    assert_eq!(info.channels, 2);
    assert_eq!(info.num_samples, AUDIO_SAMPLES);
    let mut first = Vec::new();
    a.samples_into(100_000, 4096, &mut first).expect("read");
    assert_eq!(first.len(), 4096 * info.frame_bytes());
    assert!(first.iter().any(|&x| x != 0), "sine must not be silent");
    let mut again = Vec::new();
    a.samples_into(0, 16, &mut again).expect("read start");
    a.samples_into(100_000, 4096, &mut again).expect("re-read");
    assert_eq!(first, again);
    // Past the end is silence, not an error.
    a.samples_into(AUDIO_SAMPLES - 10, 100, &mut again)
        .expect("read past end");
    assert!(again[10 * info.frame_bytes()..].iter().all(|&x| x == 0));
}

#[test]
fn index_cache_is_written_and_reused() {
    let Some(clip) = cfr_clip() else { return };
    let base = work_dir().join("cfr-index");
    let index = work_dir().join("cfr-index.0.bsindex");
    let _ = std::fs::remove_file(&index);
    let opts = BestSourceOptions {
        cache: IndexCache::File(base),
        ..BestSourceOptions::default()
    };
    let cancel = AtomicBool::new(false);
    let mut calls = 0;
    let first = BestSourceProvider::open_with_progress(clip, &opts, &cancel, |_, _| calls += 1)
        .expect("first open");
    assert!(calls > 0, "indexing reports progress");
    assert!(index.exists(), "index file written");
    let mut calls2 = 0;
    let second = BestSourceProvider::open_with_progress(clip, &opts, &cancel, |_, _| calls2 += 1)
        .expect("cached open");
    assert_eq!(calls2, 0, "cached open does not re-index");
    assert_eq!(first.info().frame_pts_ms, second.info().frame_pts_ms);
    assert_eq!(first.info().keyframes, second.info().keyframes);
}

#[test]
fn indexing_can_be_cancelled() {
    let Some(clip) = cfr_clip() else { return };
    let cancel = AtomicBool::new(false);
    let res = BestSourceProvider::open_with_progress(clip, &no_cache(), &cancel, |_, _| {
        cancel.store(true, Ordering::Relaxed);
    });
    assert!(matches!(res, Err(MediaError::Cancelled)), "{:?}", res.err());
}

#[test]
fn missing_file_is_a_clean_error() {
    let cancel = AtomicBool::new(false);
    let res = BestSourceProvider::open_with_progress(
        Path::new("C:\\definitely\\not\\here.mkv"),
        &no_cache(),
        &cancel,
        |_, _| {},
    );
    match res {
        Err(MediaError::BestSource(msg)) => assert_ne!(msg, ""),
        other => panic!("unexpected: {:?}", other.err()),
    }
}

#[test]
fn hybrid_starts_on_ffms2_then_switches_to_bestsource() {
    let Some(clip) = cfr_clip() else { return };
    let cancel = AtomicBool::new(false);
    let mut h = HybridVideo::open(clip, &no_cache(), &cancel, |_, _| {}).expect("hybrid open");
    let mut buf = Vec::new();
    if h.kind() == ProviderKind::Ffms2 {
        h.decode_frame_into(10, &mut buf).expect("decode on ffms2");
    }
    match h.wait_upgrade() {
        UpgradeStatus::Switched | UpgradeStatus::Done => {}
        other => panic!("upgrade failed: {other:?}"),
    }
    assert_eq!(h.kind(), ProviderKind::BestSource);
    assert_eq!(h.info().num_frames, CFR_FRAMES);
    h.decode_frame_into(10, &mut buf)
        .expect("decode on bestsource");
}

#[test]
fn track_list_reports_video_and_audio() {
    let Some(clip) = cfr_clip() else { return };
    let types = media::track_types(clip).expect("track list");
    assert_eq!(types, vec![0, 1]);
}
