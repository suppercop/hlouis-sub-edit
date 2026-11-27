//! Idle-time sibling pre-indexing: indexes uncached video files in the same folder at
//! `ThreadPriority::Idle`, pausing whenever the user is actively playing or decoding and
//! respecting the [`IndexStore`] byte cap.

use crate::bestsource::{track_types, BestSourceAudio, BestSourceOptions, BestSourceProvider};
use crate::ffi::win::{set_current_thread_priority, ThreadPriority};
use crate::index_store::IndexStore;
use crate::MediaError;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;

/// Video container extensions considered when scanning sibling files in a folder.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "webm", "avi", "mov", "ts", "m2ts", "m4v", "flv", "wmv",
];

fn lowercase_filename(path: &Path) -> String {
    path.file_name()
        .map_or_default(|n| n.to_string_lossy().to_lowercase())
}

/// Lists sibling video files in `current.parent()`, excluding `current` itself.
/// Files ordered after `current` alphabetically come first so the next episode in a folder
/// is pre-indexed first.
#[must_use]
pub fn list_sibling_videos(current: &Path) -> Vec<PathBuf> {
    let Some(dir) = current.parent() else {
        return Vec::new();
    };
    let current_canon = fs::canonicalize(current).unwrap_or_else(|_| current.to_path_buf());
    let current_key = current_canon.to_string_lossy().to_lowercase();
    let current_name = lowercase_filename(current);

    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut all: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_video_path(p))
        .filter(|p| {
            let p_canon = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            p_canon.to_string_lossy().to_lowercase() != current_key
        })
        .collect();
    all.sort_by_key(|p| lowercase_filename(p));
    let split = all.partition_point(|p| lowercase_filename(p) <= current_name);
    all.rotate_left(split);
    all
}

fn is_video_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// Snapshot of idle sibling pre-indexing state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PreindexProgress {
    pub current_file: Option<String>,
    pub completed_files: usize,
    pub total_files: usize,
    pub current_fraction: f32,
    pub paused: bool,
    pub finished: bool,
    pub last_error: Option<String>,
}

struct ControlState {
    paused: bool,
    cancelled: bool,
    snapshot: PreindexProgress,
}

struct Control {
    mu: Mutex<ControlState>,
    cv: Condvar,
    cancel_flag: AtomicBool,
}

impl Control {
    fn wait_while_paused(&self) -> bool {
        let mut guard = self.mu.lock().unwrap_or_else(PoisonError::into_inner);
        while guard.paused && !guard.cancelled {
            guard.snapshot.paused = true;
            guard = self.cv.wait(guard).unwrap_or_else(PoisonError::into_inner);
        }
        guard.snapshot.paused = false;
        guard.cancelled
    }
}

/// Background sibling pre-indexer running at [`ThreadPriority::Idle`].
pub struct PreindexHandle {
    ctrl: Arc<Control>,
    worker: Option<JoinHandle<()>>,
}

impl PreindexHandle {
    /// Starts pre-indexing uncached sibling video files in the same folder as `current_video`.
    /// Returns `None` if `store.max_bytes() == 0` or there are no uncached siblings.
    #[must_use]
    pub fn start(
        current_video: &Path,
        store: IndexStore,
        threads: i32,
        initially_paused: bool,
    ) -> Option<Self> {
        if store.max_bytes() == 0 {
            return None;
        }
        let targets: Vec<PathBuf> = list_sibling_videos(current_video)
            .into_iter()
            .filter(|p| !store.has_index(p))
            .collect();
        if targets.is_empty() {
            return None;
        }
        let total_files = targets.len();
        let ctrl = Arc::new(Control {
            mu: Mutex::new(ControlState {
                paused: initially_paused,
                cancelled: false,
                snapshot: PreindexProgress {
                    current_file: None,
                    completed_files: 0,
                    total_files,
                    current_fraction: 0.0,
                    paused: initially_paused,
                    finished: false,
                    last_error: None,
                },
            }),
            cv: Condvar::new(),
            cancel_flag: AtomicBool::new(false),
        });
        let worker_ctrl = Arc::clone(&ctrl);
        let current_video = current_video.to_path_buf();
        let decode_threads = if threads > 0 { threads.min(2) } else { 2 };

        let worker = std::thread::Builder::new()
            .name("bestsource-preindex".to_owned())
            .spawn(move || {
                let _ = set_current_thread_priority(ThreadPriority::Idle);
                run_preindex_loop(
                    &current_video,
                    &targets,
                    &store,
                    decode_threads,
                    &worker_ctrl,
                );
            })
            .ok()?;

        Some(Self {
            ctrl,
            worker: Some(worker),
        })
    }

    /// Pauses or resumes sibling pre-indexing (e.g., pause while the user is playing/decoding).
    pub fn set_paused(&self, paused: bool) {
        let mut guard = self.ctrl.mu.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.paused != paused {
            guard.paused = paused;
            guard.snapshot.paused = paused;
            self.ctrl.cv.notify_all();
        }
    }

    /// Cancels pre-indexing immediately (waking from pause if needed).
    pub fn cancel(&self) {
        self.ctrl.cancel_flag.store(true, Ordering::Release);
        let mut guard = self.ctrl.mu.lock().unwrap_or_else(PoisonError::into_inner);
        guard.cancelled = true;
        self.ctrl.cv.notify_all();
    }

    /// Current progress snapshot.
    #[must_use]
    pub fn snapshot(&self) -> PreindexProgress {
        self.ctrl
            .mu
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .snapshot
            .clone()
    }

    /// Whether all sibling files have been processed or cancelled.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.snapshot().finished
    }
}

impl Drop for PreindexHandle {
    fn drop(&mut self) {
        self.cancel();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

fn preindex_one_sibling(
    current_video: &Path,
    sibling: &Path,
    store: &IndexStore,
    decode_threads: i32,
    ctrl: &Control,
) -> Result<bool, MediaError> {
    let cache = store
        .cache_for(sibling)
        .map_err(|e| MediaError::BestSource(e.to_string()))?;
    let opts = BestSourceOptions {
        track: -1,
        threads: decode_threads,
        cache,
    };
    let on_progress = |cur: i64, tot: i64| {
        if cur != i64::MAX && tot > 0 {
            #[allow(clippy::cast_possible_truncation)]
            let frac = (cur as f64 / tot as f64).clamp(0.0, 1.0) as f32;
            let mut g = ctrl.mu.lock().unwrap_or_else(PoisonError::into_inner);
            g.snapshot.current_fraction = frac;
        }
        if ctrl.wait_while_paused() {
            ctrl.cancel_flag.store(true, Ordering::Release);
        }
    };
    let _video =
        BestSourceProvider::open_with_progress(sibling, &opts, &ctrl.cancel_flag, on_progress)?;
    let _ = store.touch(current_video);
    let _ = store.enforce_limit_preserving(Some(current_video));
    if !store.has_index(sibling) {
        return Ok(false);
    }
    if !ctrl.wait_while_paused()
        && track_types(sibling)
            .is_ok_and(|types| types.contains(&bestsource_sys::AVMEDIA_TYPE_AUDIO))
    {
        let _audio =
            BestSourceAudio::open_with_progress(sibling, &opts, &ctrl.cancel_flag, on_progress)?;
        let _ = store.touch(current_video);
        let _ = store.enforce_limit_preserving(Some(current_video));
        if !store.has_index(sibling) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn run_preindex_loop(
    current_video: &Path,
    targets: &[PathBuf],
    store: &IndexStore,
    decode_threads: i32,
    ctrl: &Control,
) {
    for (idx, sibling) in targets.iter().enumerate() {
        if ctrl.wait_while_paused() {
            break;
        }
        if store.has_index(sibling) {
            let mut g = ctrl.mu.lock().unwrap_or_else(PoisonError::into_inner);
            g.snapshot.completed_files = idx + 1;
            continue;
        }
        let _ = store.touch(current_video);
        let _ = store.enforce_limit_preserving(Some(current_video));
        let name = sibling.file_name().map_or_else(
            || sibling.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        {
            let mut g = ctrl.mu.lock().unwrap_or_else(PoisonError::into_inner);
            g.snapshot.current_file = Some(name.clone());
            g.snapshot.current_fraction = 0.0;
        }

        match preindex_one_sibling(current_video, sibling, store, decode_threads, ctrl) {
            Ok(keep_going) => {
                let mut g = ctrl.mu.lock().unwrap_or_else(PoisonError::into_inner);
                g.snapshot.completed_files = idx + 1;
                g.snapshot.current_fraction = 1.0;
                if !keep_going {
                    break;
                }
            }
            Err(MediaError::Cancelled) => break,
            Err(e) => {
                let mut g = ctrl.mu.lock().unwrap_or_else(PoisonError::into_inner);
                g.snapshot.completed_files = idx + 1;
                g.snapshot.last_error = Some(format!("{name}: {e}"));
            }
        }
    }

    let mut g = ctrl.mu.lock().unwrap_or_else(PoisonError::into_inner);
    g.snapshot.current_file = None;
    g.snapshot.finished = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_siblings_excluding_current_and_non_video() {
        let dir = std::env::temp_dir().join(format!("hlouis-preindex-list-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for name in ["ep01.mkv", "ep02.mkv", "ep03.mp4", "notes.txt", "ep02.ass"] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let siblings = list_sibling_videos(&dir.join("ep02.mkv"));
        let names: Vec<String> = siblings
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        // Files after ep02 come first (ep03), then earlier ones (ep01); notes.txt & ep02.ass excluded.
        assert_eq!(names, vec!["ep03.mp4", "ep01.mkv"]);
        let _ = fs::remove_dir_all(&dir);
    }
}
