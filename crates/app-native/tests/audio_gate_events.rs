//! The audio gate of the decoder opens on `ExactAudioReady`; it must be sent exactly once per
//! opened file in every indexing state (fresh, fully cached, audio-only cached, no audio).
//!
//! Needs `ffmpeg` (`HLOUIS_FFMPEG` or `PATH`); skipped with a message otherwise.

use app_native::video_player::{PlayerEvent, VideoPlayerHandle};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn make_clip(dir: &Path, name: &str, with_audio: bool) -> Option<PathBuf> {
    let ff = std::env::var("HLOUIS_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_owned());
    let out = dir.join(name);
    let mut cmd = Command::new(ff);
    cmd.args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=320x240:rate=25:duration=4");
    if with_audio {
        cmd.args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=4",
        ]);
    }
    cmd.args([
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-g",
        "25",
        "-pix_fmt",
        "yuv420p",
    ]);
    if with_audio {
        cmd.args(["-c:a", "aac", "-shortest"]);
    }
    cmd.arg(&out);
    cmd.status().ok()?.success().then_some(out)
}

/// Opens `clip` and returns (audio-ready count, audio-failed count, exact video reached) after
/// the file settled.
fn open_and_count(clip: &Path) -> (usize, usize) {
    let handle = VideoPlayerHandle::spawn(String::new(), false, 512, egui::Context::default())
        .expect("spawn");
    handle.open_video(clip.to_path_buf()).expect("open");
    let (mut ready, mut failed) = (0, 0);
    let mut exact = false;
    let t0 = Instant::now();
    let mut settled_at: Option<Instant> = None;
    while t0.elapsed() < Duration::from_secs(60) {
        for evt in handle.drain_events() {
            match evt {
                PlayerEvent::ExactAudioReady => ready += 1,
                PlayerEvent::ExactAudioFailed(_) => failed += 1,
                PlayerEvent::ExactReady { .. } => exact = true,
                PlayerEvent::Frame(_) => handle.release_frame_slots(1),
                _ => {}
            }
        }
        if (ready + failed > 0) && settled_at.is_none() {
            settled_at = Some(Instant::now());
        }
        // After the outcome arrived, keep listening a while so a duplicate would be seen.
        // (Fully cached files never emit `ExactReady`, so cap the wait.)
        if settled_at.is_some_and(|t| {
            t.elapsed() > Duration::from_secs(5) || (exact && t.elapsed() > Duration::from_secs(2))
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    (ready, failed)
}

#[test]
fn audio_outcome_is_sent_exactly_once_in_every_index_state() {
    let dir = std::env::temp_dir().join(format!("hlouis-audio-gate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let Some(clip) = make_clip(&dir, "with-audio.mkv", true) else {
        eprintln!("skipped: no ffmpeg");
        return;
    };
    let silent = make_clip(&dir, "silent.mkv", false).expect("silent clip");
    // Isolated index store (IndexStore::default_location reads LOCALAPPDATA).
    let local = dir.join("local");
    std::fs::create_dir_all(&local).expect("local");
    // SAFETY: single test in this process, no other thread reads the environment yet.
    unsafe { std::env::set_var("LOCALAPPDATA", &local) };

    // 1. Fresh index: audio and video both build in the background.
    assert_eq!(open_and_count(&clip), (1, 0), "fresh index");
    // 2. Fully cached: the worker never indexes, the outcome is known at open.
    assert_eq!(open_and_count(&clip), (1, 0), "fully cached");
    // 3. Audio-only cached: drop the video index entry, keep the audio one.
    let store = local.join("hlouis-sub-edit").join("bsindex");
    let mut removed = 0;
    for e in std::fs::read_dir(&store).expect("store").flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.ends_with(".0.bsindex") {
            std::fs::remove_file(e.path()).expect("remove video index");
            removed += 1;
        }
    }
    assert!(removed > 0, "expected a video index entry to remove");
    assert_eq!(open_and_count(&clip), (1, 0), "audio-only cached");
    // 4. File without an audio track.
    assert_eq!(open_and_count(&silent), (1, 0), "no audio track");
    let _ = std::fs::remove_dir_all(&dir);
}
