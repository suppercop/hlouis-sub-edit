//! `BestSource` provider tests on clips generated at test time with the project's LGPL
//! `ffmpeg.exe` (`.native/tools`, or `HLOUIS_FFMPEG`). No media is committed.

use media::{
    AudioProvider, AudioUpgradeStatus, BestSourceAudio, BestSourceOptions, BestSourceProvider,
    FastOpen, FfmsVideo, HybridOptions, HybridVideo, IndexCache, IndexPhase, IndexStore,
    MediaError, PartialIndex, PlanarFrame, PreindexHandle, ProviderKind, QuickVideo,
    ThreadPriority, UpgradeStatus, VideoProvider,
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

fn hybrid_opts(fast: FastOpen, store: Option<IndexStore>) -> HybridOptions {
    HybridOptions {
        fast,
        bestsource: no_cache(),
        audio_track: -1,
        index_audio: true,
        store,
        priority: ThreadPriority::BelowNormal,
    }
}

#[test]
fn hybrid_starts_fast_then_switches_to_bestsource() {
    let Some(clip) = cfr_clip() else { return };
    for fast in [FastOpen::Quick, FastOpen::Ffms2] {
        let cancel = AtomicBool::new(false);
        let mut h = HybridVideo::open(clip, &hybrid_opts(fast, None), &cancel, |_, _| {})
            .expect("hybrid open");
        let mut buf = Vec::new();
        if h.is_indexing() {
            assert!(h.index_progress().is_some());
            h.decode_frame_into(10, &mut buf)
                .expect("decode on fast path");
        }
        match h.wait_upgrade() {
            UpgradeStatus::Switched | UpgradeStatus::Done => {}
            other => panic!("upgrade failed: {other:?}"),
        }
        assert_eq!(h.kind(), ProviderKind::BestSource);
        assert!(h.is_frame_exact());
        assert!(h.index_progress().is_none());
        assert_eq!(h.info().num_frames, CFR_FRAMES);
        h.decode_frame_into(10, &mut buf)
            .expect("decode on bestsource");
    }
}

#[test]
fn hybrid_with_store_opens_exact_directly_the_second_time() {
    let Some(clip) = cfr_clip() else { return };
    let store = IndexStore::new(work_dir().join("store"), IndexStore::DEFAULT_MAX_BYTES);
    let cancel = AtomicBool::new(false);
    let opts = hybrid_opts(FastOpen::Quick, Some(store.clone()));
    let mut h = HybridVideo::open(clip, &opts, &cancel, |_, _| {}).expect("first open");
    assert_eq!(h.kind(), ProviderKind::Quick);
    assert!(!h.is_frame_exact());
    assert!(matches!(h.wait_upgrade(), UpgradeStatus::Switched));
    drop(h);
    assert!(store.has_index(clip));
    let h2 = HybridVideo::open(clip, &opts, &cancel, |_, _| {}).expect("cached open");
    assert_eq!(h2.kind(), ProviderKind::BestSource);
    assert!(!h2.is_indexing());
}

#[test]
fn quick_open_decodes_frames_matching_exact_decode_on_cfr() {
    let Some(clip) = cfr_clip() else { return };
    let mut q = QuickVideo::open(clip, -1, 0).expect("quick open");
    assert!(!q.is_frame_exact());
    assert_eq!((q.info().width, q.info().height), (320, 240));
    assert!(
        q.info().num_frames.abs_diff(CFR_FRAMES) <= 1,
        "{} frames, {} ms, fps {}/{}",
        q.info().num_frames,
        q.info().duration_ms,
        q.info().fps_num,
        q.info().fps_den
    );
    let mut bs = open_bs(clip);
    let (mut a, mut b) = (Vec::new(), Vec::new());
    // Sequential, forward skip, backward seek and long seek.
    for n in [0, 1, 2, 30, 31, 200, 5, 120, 121, 239] {
        let pts = q.decode_frame_into(n, &mut a).expect("quick decode");
        bs.decode_frame_into(n, &mut b).expect("exact decode");
        assert_eq!(pts, bs.info().frame_pts_ms[n], "frame {n} pts");
        assert_eq!(checksum(&a), checksum(&b), "frame {n} pixels");
    }
}

#[test]
fn track_list_reports_video_and_audio() {
    let Some(clip) = cfr_clip() else { return };
    let types = media::track_types(clip).expect("track list");
    assert_eq!(types, vec![0, 1]);
}

#[test]
fn progressive_exact_seeking_matches_bestsource_and_switches_seamlessly() {
    let Some(cfr) = cfr_clip() else { return };
    let Some(vfr) = vfr_clip() else { return };
    for (clip, check_frame) in [(cfr, 60usize), (vfr, 72usize)] {
        let mut bs_truth = open_bs(clip);
        let mut truth_rgba = Vec::new();
        let truth_pts = bs_truth
            .decode_frame_into(check_frame, &mut truth_rgba)
            .expect("truth decode");

        let partial = PartialIndex::new().expect("partial index");
        let mut q =
            QuickVideo::open_with_partial(clip, -1, 2, Some(partial.clone())).expect("quick");
        assert!(!q.is_frame_exact());
        assert!(!q.is_frame_exact_at(check_frame));

        let cancel = AtomicBool::new(false);
        let saw_partial_exact = std::sync::atomic::AtomicBool::new(false);
        let mut q_rgba = Vec::new();
        let _bs = BestSourceProvider::open_with_partial_and_progress(
            clip,
            &no_cache(),
            Some(&partial),
            &cancel,
            |_cur, _tot| {
                if !saw_partial_exact.load(Ordering::Relaxed) && partial.is_exact_at(check_frame) {
                    assert!(q.is_frame_exact_at(check_frame));
                    let pts = q
                        .decode_frame_into(check_frame, &mut q_rgba)
                        .expect("progressive exact decode");
                    assert_eq!(
                        pts, truth_pts,
                        "progressive PTS mismatch at frame {check_frame}"
                    );
                    assert_eq!(
                        checksum(&q_rgba),
                        checksum(&truth_rgba),
                        "progressive pixel mismatch at frame {check_frame}"
                    );
                    saw_partial_exact.store(true, Ordering::Relaxed);
                }
            },
        )
        .expect("index with partial");
        assert!(saw_partial_exact.load(Ordering::Relaxed));
        assert!(q.is_frame_exact_at(check_frame));
        assert!(q.next_keyframe(0).is_some());
    }
}

#[test]
fn hybrid_video_indexes_audio_first_then_video_and_handles_no_audio() {
    let Some(cfr) = cfr_clip() else { return };
    let Some(vfr) = vfr_clip() else { return };
    let store_dir = work_dir().join("audio-first-store");
    let _ = std::fs::remove_dir_all(&store_dir);
    let store = IndexStore::new(store_dir, IndexStore::DEFAULT_MAX_BYTES);
    let cancel = AtomicBool::new(false);
    let mut h = HybridVideo::open(
        cfr,
        &hybrid_opts(FastOpen::Quick, Some(store.clone())),
        &cancel,
        |_, _| {},
    )
    .expect("hybrid open");

    // The first phase shown to the user is the audio index.
    assert_eq!(
        h.index_progress().map(|p| p.phase),
        Some(IndexPhase::Audio),
        "audio must be indexed first"
    );

    let mut audio_ready = false;
    let mut video_switched = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline && !(audio_ready && video_switched) {
        if !audio_ready {
            match h.poll_audio() {
                AudioUpgradeStatus::Ready => audio_ready = true,
                AudioUpgradeStatus::Failed(e) => panic!("audio upgrade failed: {e}"),
                AudioUpgradeStatus::NoAudio => panic!("expected audio track on cfr clip"),
                AudioUpgradeStatus::Pending => {}
            }
        }
        if !video_switched {
            match h.poll_upgrade() {
                UpgradeStatus::Switched => {
                    video_switched = true;
                    assert_eq!(h.kind(), ProviderKind::BestSource);
                    assert!(h.is_frame_exact());
                    // Audio is indexed before video, so it can never still be pending here.
                    assert!(
                        !matches!(h.poll_audio(), AudioUpgradeStatus::Pending),
                        "audio must be ready before the video upgrade"
                    );
                    audio_ready = true;
                }
                UpgradeStatus::Failed(e) => panic!("video upgrade failed: {e}"),
                UpgradeStatus::Pending | UpgradeStatus::Done => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(audio_ready, "audio index must land");
    assert!(video_switched, "video must switch to exact");
    let mut aud = h.take_audio().expect("audio provider ready");
    let mut pcm = Vec::new();
    aud.samples_into(0, 1024, &mut pcm)
        .expect("read audio samples");
    assert_eq!(pcm.len(), 1024 * aud.audio_info().frame_bytes());
    drop(h);

    // Silent VFR clip reports NoAudio cleanly (never an error).
    let mut hv = HybridVideo::open(
        vfr,
        &hybrid_opts(FastOpen::Quick, Some(store)),
        &cancel,
        |_, _| {},
    )
    .expect("open silent vfr");
    assert!(matches!(hv.wait_upgrade(), UpgradeStatus::Switched));
    assert!(matches!(hv.poll_audio(), AudioUpgradeStatus::NoAudio));
}

#[test]
fn corrupt_cache_files_rebuild_safely_and_never_crash() {
    let Some(clip) = cfr_clip() else { return };
    let store_dir = work_dir().join("corrupt-store");
    let _ = std::fs::remove_dir_all(&store_dir);
    let store = IndexStore::new(store_dir, IndexStore::DEFAULT_MAX_BYTES);
    let cancel = AtomicBool::new(false);

    // First build valid video + audio cache entries.
    let mut h = HybridVideo::open(
        clip,
        &hybrid_opts(FastOpen::Quick, Some(store.clone())),
        &cancel,
        |_, _| {},
    )
    .expect("initial open");
    assert!(matches!(h.wait_upgrade(), UpgradeStatus::Switched));
    drop(h);

    let entries = store.entries_for(clip).expect("entries");
    assert_ne!(entries.len(), 0);

    // Corrupt every .bsindex with 0 bytes, truncated bytes, and random garbage.
    for payload in [Vec::new(), b"BS2V\x00\x00".to_vec(), vec![0xDEu8; 256]] {
        for p in &entries {
            std::fs::write(p, &payload).expect("write corrupt cache");
        }
        // BestSourceProvider direct open rebuilds cleanly.
        let opts = BestSourceOptions {
            cache: store.cache_for(clip).expect("cache"),
            ..BestSourceOptions::default()
        };
        let bs = BestSourceProvider::open_with_progress(clip, &opts, &cancel, |_, _| {})
            .expect("rebuild from corrupt cache");
        assert_eq!(bs.info().num_frames, CFR_FRAMES);
        drop(bs);

        // Corrupt again and test HybridVideo: must NOT hang or crash, opens Quick first and rebuilds!
        for p in &entries {
            std::fs::write(p, &payload).expect("write corrupt cache");
        }
        let mut h2 = HybridVideo::open(
            clip,
            &hybrid_opts(FastOpen::Quick, Some(store.clone())),
            &cancel,
            |_, _| {},
        )
        .expect("hybrid open with corrupt cache");
        assert_eq!(h2.kind(), ProviderKind::Quick);
        assert!(matches!(h2.wait_upgrade(), UpgradeStatus::Switched));
        assert_eq!(h2.kind(), ProviderKind::BestSource);
        assert!(matches!(h2.poll_audio(), AudioUpgradeStatus::Ready));
    }
}

#[test]
fn sibling_preindexer_pauses_resumes_and_respects_cache_cap() {
    let Some(clip) = cfr_clip() else { return };
    let dir = work_dir().join("siblings");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let ep1 = dir.join("ep01.mkv");
    let ep2 = dir.join("ep02.mkv");
    let ep3 = dir.join("ep03.mkv");
    std::fs::copy(clip, &ep1).unwrap();
    std::fs::copy(clip, &ep2).unwrap();
    std::fs::copy(clip, &ep3).unwrap();
    std::fs::write(dir.join("readme.txt"), b"ignore me").unwrap();

    let store = IndexStore::new(
        work_dir().join("sibling-store"),
        IndexStore::DEFAULT_MAX_BYTES,
    );
    let cancel = AtomicBool::new(false);
    // Index ep1 first as the currently open video.
    let mut h1 = HybridVideo::open(
        &ep1,
        &hybrid_opts(FastOpen::Quick, Some(store.clone())),
        &cancel,
        |_, _| {},
    )
    .expect("open ep1");
    h1.wait_upgrade();
    drop(h1);
    assert!(store.has_index(&ep1));
    assert!(!store.has_index(&ep2));

    // Start pre-indexer initially paused, ensure it stays paused, then unpause to finish.
    let pre = PreindexHandle::start(&ep1, store.clone(), 2, true).expect("start preindexer");
    std::thread::sleep(std::time::Duration::from_millis(120));
    assert!(pre.snapshot().paused);
    assert!(!pre.is_finished());

    pre.set_paused(false);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !pre.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(pre.is_finished());
    drop(pre);
    assert!(store.has_index(&ep2));
    assert!(store.has_index(&ep3));

    // Opening ep2 now loads directly from the pre-indexed cache.
    let h2 = HybridVideo::open(
        &ep2,
        &hybrid_opts(FastOpen::Quick, Some(store.clone())),
        &cancel,
        |_, _| {},
    )
    .expect("open preindexed ep2");
    assert_eq!(h2.kind(), ProviderKind::BestSource);
    assert!(!h2.is_indexing());
    drop(h2);

    // Tight cache cap: keep ep1 protected and respect total_bytes <= cap.
    let ep1_bytes: u64 = store
        .entries_for(&ep1)
        .unwrap()
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum();
    let mut tight_store = store.clone();
    tight_store.set_max_bytes(ep1_bytes + 512);
    let _ = tight_store.enforce_limit_preserving(Some(&ep1));
    assert!(tight_store.has_index(&ep1));
    assert!(tight_store.total_bytes().unwrap() <= ep1_bytes + 512);
}

#[test]
fn bestsource_error_paths_fall_back_with_visible_message() {
    let dir = work_dir().join("fallback");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // 1. Unopenable invalid file returns descriptive error (never silent).
    let garbage = dir.join("not_a_video.mkv");
    std::fs::write(&garbage, b"this is not valid media").unwrap();
    let cancel = AtomicBool::new(false);
    let Err(err) = HybridVideo::open(
        &garbage,
        &hybrid_opts(FastOpen::Quick, None),
        &cancel,
        |_, _| {},
    ) else {
        panic!("garbage file must error");
    };
    assert_ne!(err.to_string(), "");

    // 2. Broken audio track in a valid video container surfaces AudioUpgradeStatus::Failed visibly.
    let Some(cfr) = cfr_clip() else { return };
    let mut bad_audio_opts = hybrid_opts(FastOpen::Quick, None);
    bad_audio_opts.audio_track = 999; // invalid explicit stream index
    let mut h = HybridVideo::open(cfr, &bad_audio_opts, &cancel, |_, _| {})
        .expect("open with bad audio track");
    assert!(matches!(h.wait_upgrade(), UpgradeStatus::Switched));
    match h.poll_audio() {
        AudioUpgradeStatus::Failed(e) => assert_ne!(e.to_string(), ""),
        other => panic!("expected visible audio failure, got {other:?}"),
    }

    // 3. Invalid video track: QuickVideo fails -> falls back to FFMS2 with visible notice,
    // and background BestSource failure surfaces UpgradeStatus::Failed while staying on FFMS2.
    let mut bad_track_opts = hybrid_opts(FastOpen::Quick, None);
    bad_track_opts.bestsource.track = 999;
    let mut h_ff = HybridVideo::open(cfr, &bad_track_opts, &cancel, |_, _| {})
        .expect("fallback to FFMS2 when quick track fails");
    assert_eq!(h_ff.kind(), ProviderKind::Ffms2);
    assert!(
        h_ff.take_fallback_notice().is_some(),
        "fallback to FFMS2 must emit visible notice"
    );
    match h_ff.wait_upgrade() {
        UpgradeStatus::Failed(e) => assert_ne!(e.to_string(), ""),
        other => panic!("expected visible BestSource track failure, got {other:?}"),
    }
    assert_eq!(h_ff.kind(), ProviderKind::Ffms2);

    // 4. Quick succeeds while BestSource fails (unwritable fatal cache file path): stays on
    // QuickVideo with visible UpgradeStatus::Failed message and continues decoding frames.
    let mut unwritable_opts = hybrid_opts(FastOpen::Quick, None);
    unwritable_opts.bestsource.cache = IndexCache::File(cfr.join("impossible_subdir"));
    let mut h_q = HybridVideo::open(cfr, &unwritable_opts, &cancel, |_, _| {})
        .expect("open quick before bestsource cache write failure");
    assert_eq!(h_q.kind(), ProviderKind::Quick);
    match h_q.wait_upgrade() {
        UpgradeStatus::Failed(e) => assert_ne!(e.to_string(), ""),
        other => panic!("expected visible BestSource failure, got {other:?}"),
    }
    assert_eq!(h_q.kind(), ProviderKind::Quick);
    let mut buf = Vec::new();
    h_q.decode_frame_into(0, &mut buf)
        .expect("quick still decodes after fallback");
    assert_ne!(buf.len(), 0);

    // 5. open_cached_only on missing cache fails cleanly.
    let copy = dir.join("ep_disappears.mkv");
    std::fs::copy(cfr, &copy).unwrap();
    let miss = BestSourceProvider::open_cached_only(&copy, &no_cache());
    assert!(matches!(miss, Err(MediaError::BestSource(_))));
}
