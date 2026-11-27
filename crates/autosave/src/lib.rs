//! Crash-safe autosave, autobackup, session-lock crash recovery, and off-UI-thread worker.
//!
//! Reference behaviour (Aegisub 3.5.1 `src/subs_controller.cpp` & `src/auto4_base.cpp`):
//! - Autosave files are named `<stem>.AUTOSAVE.<timestamp>.ass` inside a caller-supplied base
//!   directory (never hardcoded to `%APPDATA%`); the newest `max_keep` files per stem are kept.
//! - Autobackup copies the prior saved file to `<stem>.ORIGINAL.ass` in the backup directory.
//! - All writes use write-to-temp in the same directory + `flush()` + `sync_all()` + atomic
//!   `std::fs::rename`, preventing half-written files on crash or power loss.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Typed errors returned by `autosave` operations.
#[derive(Debug, Error)]
pub enum AutosaveError {
    #[error("target path is read-only or permission denied: {path}")]
    ReadOnlyTarget {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("insufficient disk space writing {path}")]
    DiskFull {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("autosave operation was cancelled")]
    Cancelled,
    #[error("autosave worker thread failed to spawn: {0}")]
    WorkerSpawn(#[source] std::io::Error),
    #[error("autosave worker thread is no longer running")]
    WorkerDisconnected,
    #[error("the autosave was written, but old autosave {path} could not be removed: {source}")]
    PruneFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the system clock is set before 1970-01-01; cannot name the autosave file")]
    ClockBeforeEpoch,
}

fn classify_io_err(path: PathBuf, err: std::io::Error) -> AutosaveError {
    match err.kind() {
        ErrorKind::PermissionDenied => AutosaveError::ReadOnlyTarget { path, source: err },
        ErrorKind::StorageFull => AutosaveError::DiskFull { path, source: err },
        _ => {
            // Check POSIX ENOSPC (28) and Windows ERROR_DISK_FULL (112) as fallback.
            if matches!(err.raw_os_error(), Some(28 | 112)) {
                AutosaveError::DiskFull { path, source: err }
            } else {
                AutosaveError::Io { path, source: err }
            }
        }
    }
}

/// Configuration for autosave and autobackup directories and retention.
#[derive(Debug, Clone)]
pub struct AutosaveConfig {
    /// Directory where `<stem>.AUTOSAVE.<ts>.ass` and session lock markers are stored.
    pub autosave_dir: PathBuf,
    /// Directory where `<stem>.ORIGINAL.ass` autobackups are stored.
    pub backup_dir: PathBuf,
    /// Autosave interval in seconds (used by UI schedulers / worker timers).
    pub interval: Duration,
    /// Maximum number of autosave files retained per document stem (`>= 1`).
    pub max_keep: usize,
}

impl AutosaveConfig {
    /// Creates a configuration rooted under `base_dir/autosave` and `base_dir/autoback`.
    #[must_use]
    pub fn from_base_dir(base_dir: &Path) -> Self {
        Self {
            autosave_dir: base_dir.join("autosave"),
            backup_dir: base_dir.join("autoback"),
            interval: Duration::from_secs(60),
            max_keep: 10,
        }
    }
}

/// Crash-safely writes `bytes` to `dest` by writing a sibling temp file, flushing, calling
/// `sync_all()`, and atomically renaming over `dest`.
///
/// # Errors
/// Returns an [`AutosaveError`] on I/O, permission, disk-full, or worker failures.
pub fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<(), AutosaveError> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| classify_io_err(parent.to_path_buf(), e))?;

    // Fail fast if an existing target is marked read-only.
    if let Ok(meta) = fs::metadata(dest) {
        if meta.permissions().readonly() {
            return Err(AutosaveError::ReadOnlyTarget {
                path: dest.to_path_buf(),
                source: std::io::Error::new(ErrorKind::PermissionDenied, "target is read-only"),
            });
        }
    }

    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = dest
        .file_name()
        .map_or_else(|| "file".to_owned(), |n| n.to_string_lossy().into_owned());
    let temp_path = parent.join(format!(".{file_name}.tmp-{pid}-{seq}"));

    let write_res = (|| -> Result<(), std::io::Error> {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        f.write_all(bytes)?;
        f.flush()?;
        f.sync_all()?;
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = fs::remove_file(&temp_path);
        return Err(classify_io_err(temp_path, err));
    }

    if let Err(err) = fs::rename(&temp_path, dest) {
        let _ = fs::remove_file(&temp_path);
        return Err(classify_io_err(dest.to_path_buf(), err));
    }

    Ok(())
}

/// Writes one autosave snapshot `<stem>.AUTOSAVE.<epoch_ms>-<seq>.ass` and prunes older
/// autosaves for `stem` beyond `max_keep`.
///
/// # Errors
/// Returns an [`AutosaveError`] on I/O, permission, disk-full, or worker failures.
pub fn write_autosave_now(
    autosave_dir: &Path,
    doc_stem: &str,
    bytes: &[u8],
    max_keep: usize,
    cancel: Option<&AtomicBool>,
) -> Result<PathBuf, AutosaveError> {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
        return Err(AutosaveError::Cancelled);
    }
    let safe_stem = sanitize_stem(doc_stem);
    let ts = timestamp_tag()?;
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let file_name = format!("{safe_stem}.AUTOSAVE.{ts}-{seq:04}.ass");
    let target = autosave_dir.join(file_name);

    write_atomic(&target, bytes)?;
    prune_old_autosaves(autosave_dir, &safe_stem, max_keep.max(1))?;
    Ok(target)
}

/// Creates an Aegisub-style `<stem>.ORIGINAL.ass` autobackup of `saved_file` inside `backup_dir`.
///
/// Copies the existing `saved_file` before an overwrite save so the original version is preserved.
///
/// # Errors
/// Returns an [`AutosaveError`] on I/O, permission, disk-full, or worker failures.
pub fn create_autobackup(saved_file: &Path, backup_dir: &Path) -> Result<PathBuf, AutosaveError> {
    let bytes = fs::read(saved_file).map_err(|e| classify_io_err(saved_file.to_path_buf(), e))?;
    let stem = saved_file.file_stem().map_or_else(
        || "untitled".to_owned(),
        |s| s.to_string_lossy().into_owned(),
    );
    let safe_stem = sanitize_stem(&stem);
    let dest = backup_dir.join(format!("{safe_stem}.ORIGINAL.ass"));
    write_atomic(&dest, &bytes)?;
    Ok(dest)
}

/// Active session lock marker (`<stem>.session.lock`) used to detect unclean exits / crashes.
pub struct SessionLock {
    lock_path: PathBuf,
}

impl SessionLock {
    /// Acquires a session marker file in `autosave_dir` for `doc_stem`.
    /// Returns `(lock, had_stale_lock)` where `had_stale_lock` is `true` if a prior session crashed.
    ///
    /// # Errors
    /// Returns an [`AutosaveError`] on I/O, permission, disk-full, or worker failures.
    pub fn acquire(autosave_dir: &Path, doc_stem: &str) -> Result<(Self, bool), AutosaveError> {
        fs::create_dir_all(autosave_dir)
            .map_err(|e| classify_io_err(autosave_dir.to_path_buf(), e))?;
        let lock_path = autosave_dir.join(format!("{}.session.lock", sanitize_stem(doc_stem)));
        let had_stale = lock_path.exists();
        let marker = format!("pid={}\nts={}\n", std::process::id(), timestamp_tag()?);
        write_atomic(&lock_path, marker.as_bytes())?;
        Ok((Self { lock_path }, had_stale))
    }

    /// Path to the session lock file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.lock_path
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        // Drop cannot return an error; a stale lock only triggers the (visible) recovery prompt
        // next start, so report it on stderr instead of ignoring it.
        if let Err(e) = fs::remove_file(&self.lock_path) {
            if e.kind() != ErrorKind::NotFound {
                eprintln!(
                    "autosave: could not remove session lock {}: {e}",
                    self.lock_path.display()
                );
            }
        }
    }
}

/// Candidate autosave file discovered for crash recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryCandidate {
    pub path: PathBuf,
    pub modified: SystemTime,
}

/// Result of scanning for recoverable autosaves on startup.
#[derive(Debug, Clone, Default)]
pub struct RecoveryReport {
    /// `true` if a stale `.session.lock` file was present from an unclean exit.
    pub unclean_shutdown_detected: bool,
    /// Valid autosave files newer than `saved_file` (sorted newest-first), ignoring `.tmp-*` files.
    pub candidates: Vec<RecoveryCandidate>,
    /// Human-readable reasons for autosave files that could not be inspected (never silently skipped),
    /// plus a note when the saved file's modification time could not be read.
    pub warnings: Vec<String>,
}

/// Scans `autosave_dir` for autosaves matching `doc_stem` that are newer than `saved_file`
/// (or all valid autosaves if `saved_file` is `None` or missing).
///
/// # Errors
/// Returns an [`AutosaveError`] on I/O, permission, disk-full, or worker failures.
pub fn discover_recoveries(
    autosave_dir: &Path,
    doc_stem: &str,
    saved_file: Option<&Path>,
) -> Result<RecoveryReport, AutosaveError> {
    let safe_stem = sanitize_stem(doc_stem);
    let lock_path = autosave_dir.join(format!("{safe_stem}.session.lock"));
    let unclean_shutdown_detected = lock_path.exists();

    if !autosave_dir.exists() {
        return Ok(RecoveryReport {
            unclean_shutdown_detected,
            candidates: Vec::new(),
            warnings: Vec::new(),
        });
    }

    let mut warnings = Vec::new();
    let saved_mtime = match saved_file {
        None => None,
        Some(p) => match fs::metadata(p).and_then(|m| m.modified()) {
            Ok(t) => Some(t),
            // A saved file that does not exist yet (never saved) is a normal state, not a problem.
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => {
                warnings.push(format!(
                    "could not read the modification time of {} ({e}); offering every autosave",
                    p.display()
                ));
                None
            }
        },
    };

    let prefix = format!("{safe_stem}.AUTOSAVE.");
    let mut candidates = Vec::new();
    let entries =
        fs::read_dir(autosave_dir).map_err(|e| classify_io_err(autosave_dir.to_path_buf(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| classify_io_err(autosave_dir.to_path_buf(), e))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.')
            || !name.starts_with(&prefix)
            || !Path::new(name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("ass"))
        {
            continue;
        }
        let mtime = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(e) => {
                warnings.push(format!(
                    "could not inspect autosave {} ({e}); it was not offered for recovery",
                    path.display()
                ));
                continue;
            }
        };
        if saved_mtime.is_none_or(|sm| mtime > sm) {
            candidates.push(RecoveryCandidate {
                path,
                modified: mtime,
            });
        }
    }

    // Sort newest first (tie-break by filename descending).
    candidates.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| b.path.cmp(&a.path))
    });

    Ok(RecoveryReport {
        unclean_shutdown_detected,
        candidates,
        warnings,
    })
}

/// Background worker running autosave I/O off the UI thread.
pub struct AutosaveWorker {
    tx: mpsc::Sender<WorkerMsg>,
    rx: mpsc::Receiver<Result<PathBuf, AutosaveError>>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

enum WorkerMsg {
    SaveBytes {
        doc_stem: String,
        bytes: Vec<u8>,
    },
    SaveClosure {
        doc_stem: String,
        produce: Box<dyn FnOnce() -> Vec<u8> + Send>,
    },
    Shutdown,
}

impl AutosaveWorker {
    /// Spawns the background autosave worker thread.
    ///
    /// # Errors
    /// Returns an [`AutosaveError`] on I/O, permission, disk-full, or worker failures.
    pub fn spawn(config: AutosaveConfig) -> Result<Self, AutosaveError> {
        let (tx, job_rx) = mpsc::channel::<WorkerMsg>();
        let (res_tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_clone = Arc::clone(&cancel);

        let handle = thread::Builder::new()
            .name("hlouis-autosave".to_owned())
            .spawn(move || {
                while let Ok(msg) = job_rx.recv() {
                    match msg {
                        WorkerMsg::Shutdown => break,
                        WorkerMsg::SaveBytes { doc_stem, bytes } => {
                            let res = write_autosave_now(
                                &config.autosave_dir,
                                &doc_stem,
                                &bytes,
                                config.max_keep,
                                Some(&cancel_clone),
                            );
                            let _ = res_tx.send(res);
                        }
                        WorkerMsg::SaveClosure { doc_stem, produce } => {
                            if cancel_clone.load(Ordering::Relaxed) {
                                let _ = res_tx.send(Err(AutosaveError::Cancelled));
                                continue;
                            }
                            let bytes = produce();
                            let res = write_autosave_now(
                                &config.autosave_dir,
                                &doc_stem,
                                &bytes,
                                config.max_keep,
                                Some(&cancel_clone),
                            );
                            let _ = res_tx.send(res);
                        }
                    }
                }
            })
            .map_err(AutosaveError::WorkerSpawn)?;

        Ok(Self {
            tx,
            rx,
            cancel,
            handle: Some(handle),
        })
    }

    /// Hands off a serialized byte snapshot to the background worker (non-blocking, < 1 ms).
    ///
    /// # Errors
    /// Returns an [`AutosaveError`] on I/O, permission, disk-full, or worker failures.
    pub fn enqueue_bytes(
        &self,
        doc_stem: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Result<(), AutosaveError> {
        self.cancel.store(false, Ordering::Relaxed);
        self.tx
            .send(WorkerMsg::SaveBytes {
                doc_stem: doc_stem.into(),
                bytes,
            })
            .map_err(|_| AutosaveError::WorkerDisconnected)
    }

    /// Hands off a snapshot-producing closure to run on the background worker thread.
    ///
    /// # Errors
    /// Returns an [`AutosaveError`] on I/O, permission, disk-full, or worker failures.
    pub fn enqueue_snapshot<F>(
        &self,
        doc_stem: impl Into<String>,
        produce: F,
    ) -> Result<(), AutosaveError>
    where
        F: FnOnce() -> Vec<u8> + Send + 'static,
    {
        self.cancel.store(false, Ordering::Relaxed);
        self.tx
            .send(WorkerMsg::SaveClosure {
                doc_stem: doc_stem.into(),
                produce: Box::new(produce),
            })
            .map_err(|_| AutosaveError::WorkerDisconnected)
    }

    /// Signals cancellation of pending/in-flight autosave jobs.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Non-blocking drain of completed autosave results.
    #[must_use]
    pub fn drain_results(&self) -> Vec<Result<PathBuf, AutosaveError>> {
        let mut out = Vec::new();
        while let Ok(r) = self.rx.try_recv() {
            out.push(r);
        }
        out
    }

    /// Blocks until the next autosave result arrives.
    ///
    /// # Errors
    /// Returns an [`AutosaveError`] on I/O, permission, disk-full, or worker failures.
    pub fn recv_result(&self) -> Result<PathBuf, AutosaveError> {
        self.rx
            .recv()
            .map_err(|_| AutosaveError::WorkerDisconnected)?
    }
}

impl Drop for AutosaveWorker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.tx.send(WorkerMsg::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn prune_old_autosaves(
    autosave_dir: &Path,
    safe_stem: &str,
    max_keep: usize,
) -> Result<(), AutosaveError> {
    let prefix = format!("{safe_stem}.AUTOSAVE.");
    let mut files: Vec<PathBuf> = Vec::new();

    for entry in
        fs::read_dir(autosave_dir).map_err(|e| classify_io_err(autosave_dir.to_path_buf(), e))?
    {
        let entry = entry.map_err(|e| classify_io_err(autosave_dir.to_path_buf(), e))?;
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if !name.starts_with('.')
                && name.starts_with(&prefix)
                && Path::new(name)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("ass"))
            {
                files.push(path);
            }
        }
    }

    // Filenames contain monotonically increasing epoch-ms + seq, so lexicographical sort = oldest first.
    files.sort();
    if files.len() > max_keep {
        let to_remove = files.len() - max_keep;
        let mut first_failure: Option<AutosaveError> = None;
        for old in &files[..to_remove] {
            if let Err(e) = fs::remove_file(old) {
                if first_failure.is_none() {
                    first_failure = Some(AutosaveError::PruneFailed {
                        path: old.clone(),
                        source: e,
                    });
                }
            }
        }
        if let Some(err) = first_failure {
            return Err(err);
        }
    }
    Ok(())
}

fn sanitize_stem(stem: &str) -> String {
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() {
        "untitled".to_owned()
    } else {
        cleaned
    }
}

fn timestamp_tag() -> Result<String, AutosaveError> {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AutosaveError::ClockBeforeEpoch)?;
    Ok(format!("{:013}", dur.as_millis()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_test_dir(name: &str) -> PathBuf {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "hlouis_autosave_test_{name}_{}_{seq}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn autosave_prunes_older_files_and_autobackup_creates_original() {
        let base = temp_test_dir("prune_backup");
        let cfg = AutosaveConfig {
            max_keep: 3,
            ..AutosaveConfig::from_base_dir(&base)
        };

        for i in 0..5 {
            let payload = format!("version {i}");
            let path = write_autosave_now(
                &cfg.autosave_dir,
                "ep01",
                payload.as_bytes(),
                cfg.max_keep,
                None,
            )
            .expect("autosave");
            assert!(path.exists());
        }

        let report = discover_recoveries(&cfg.autosave_dir, "ep01", None).expect("discover");
        assert_eq!(
            report.candidates.len(),
            3,
            "must retain only last 3 autosaves"
        );
        let newest = fs::read_to_string(&report.candidates[0].path).expect("read newest");
        assert_eq!(newest, "version 4");

        // Autobackup copies saved file to <stem>.ORIGINAL.ass.
        let saved = base.join("ep01.ass");
        write_atomic(&saved, b"original content").expect("write saved");
        let bak = create_autobackup(&saved, &cfg.backup_dir).expect("autobackup");
        assert!(bak.ends_with("ep01.ORIGINAL.ass"));
        assert_eq!(fs::read(&bak).expect("read bak"), b"original content");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn crash_recovery_detects_stale_lock_ignores_tmp_and_worker_runs_off_thread() {
        let base = temp_test_dir("recovery_worker");
        let cfg = AutosaveConfig::from_base_dir(&base);

        // Simulate a saved file, then a newer autosave + a stale half-written .tmp file + stale lock.
        let saved = base.join("ep02.ass");
        write_atomic(&saved, b"old saved").expect("write saved");
        std::thread::sleep(Duration::from_millis(20));

        let (lock, had_stale) = SessionLock::acquire(&cfg.autosave_dir, "ep02").expect("lock");
        assert!(!had_stale);
        // Simulate crash by forgetting the lock (leaving lock file on disk).
        let lock_path = lock.path().to_path_buf();
        std::mem::forget(lock);

        // Write half-written temp file that must be ignored by recovery.
        let junk_tmp = cfg.autosave_dir.join(".ep02.AUTOSAVE.tmp-999-0");
        fs::File::create(&junk_tmp).expect("create tmp");

        let worker = AutosaveWorker::spawn(cfg.clone()).expect("spawn worker");
        worker
            .enqueue_bytes("ep02", b"recovered content".to_vec())
            .expect("enqueue");
        let saved_auto = worker.recv_result().expect("worker result");
        assert!(saved_auto.exists());

        let report =
            discover_recoveries(&cfg.autosave_dir, "ep02", Some(&saved)).expect("discover");
        assert!(report.unclean_shutdown_detected);
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.candidates[0].path, saved_auto);

        let _ = fs::remove_file(lock_path);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn read_only_target_returns_typed_error() {
        let base = temp_test_dir("readonly");
        let target = base.join("locked.ass");
        write_atomic(&target, b"initial").expect("initial");
        let mut perms = fs::metadata(&target).expect("meta").permissions();
        perms.set_readonly(true);
        fs::set_permissions(&target, perms).expect("set readonly");

        let err = write_atomic(&target, b"overwrite");
        assert!(matches!(err, Err(AutosaveError::ReadOnlyTarget { .. })));

        // Restore write permission so cleanup succeeds.
        let mut perms2 = fs::metadata(&target).expect("meta2").permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms2.set_readonly(false);
        let _ = fs::set_permissions(&target, perms2);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn prune_failure_is_reported_not_swallowed() {
        let dir = temp_test_dir("prune_fail");
        // A directory squatting on an old autosave name cannot be removed with remove_file.
        let blocker = dir.join("doc.AUTOSAVE.0000000000001-0000.ass");
        fs::create_dir_all(&blocker).unwrap();
        let res = write_autosave_now(&dir, "doc", b"x", 1, None);
        assert!(matches!(res, Err(AutosaveError::PruneFailed { .. })), "{res:?}");
        let msg = res.unwrap_err().to_string();
        assert!(msg.contains("could not be removed"), "{msg}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recovery_report_carries_warnings_field() {
        let dir = temp_test_dir("recovery_warn");
        let report = discover_recoveries(&dir, "doc", None).unwrap();
        assert!(report.warnings.is_empty());
        assert!(report.candidates.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
