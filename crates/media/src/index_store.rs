//! Persistent `BestSource` index cache: one entry per video keyed by path + size + mtime + a
//! partial content hash, in the app cache directory, with a size limit and LRU cleanup.

use crate::bestsource::IndexCache;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SAMPLE: u64 = 64 * 1024;
const SUFFIX: &str = ".bsindex";

/// Directory of `BestSource` index files with an upper size bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStore {
    dir: PathBuf,
    max_bytes: u64,
}

impl IndexStore {
    /// Default limit: 2 GiB (a 24-min 1080p episode needs about 1.5 MB).
    pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

    #[must_use]
    pub fn new(dir: PathBuf, max_bytes: u64) -> Self {
        Self { dir, max_bytes }
    }

    /// `%LOCALAPPDATA%\hlouis-sub-edit\bsindex` (with `%USERPROFILE%\AppData\Local` fallback)
    /// with the default limit.
    #[must_use]
    pub fn default_location() -> Option<Self> {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .map(|p| PathBuf::from(p).join("AppData").join("Local"))
            })?;
        Some(Self::new(
            base.join("hlouis-sub-edit").join("bsindex"),
            Self::DEFAULT_MAX_BYTES,
        ))
    }

    /// Configured maximum cache size in bytes.
    #[must_use]
    pub const fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// Updates the maximum cache size in bytes.
    pub fn set_max_bytes(&mut self, max_bytes: u64) {
        self.max_bytes = max_bytes;
    }

    /// Current total size in bytes of `.bsindex` files in this store.
    pub fn total_bytes(&self) -> io::Result<u64> {
        Ok(self.list()?.iter().map(|(_, len, _)| *len).sum())
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Stable key for `video`: changes when the file is replaced, resized, touched or edited.
    pub fn key_for(video: &Path) -> io::Result<String> {
        let abs = fs::canonicalize(video).unwrap_or_else(|_| video.to_path_buf());
        let meta = fs::metadata(&abs)?;
        let size = meta.len();
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let mut h = Fnv::new();
        h.write(abs.to_string_lossy().to_lowercase().as_bytes());
        h.write(&size.to_le_bytes());
        h.write(&mtime.to_le_bytes());
        let mut f = File::open(&abs)?;
        let mut buf = vec![0u8; usize::try_from(SAMPLE).unwrap_or(65_536)];
        for off in [0, size / 2, size.saturating_sub(SAMPLE)] {
            f.seek(SeekFrom::Start(off))?;
            let n = read_up_to(&mut f, &mut buf)?;
            h.write(&buf[..n]);
        }
        Ok(format!("{:016x}-{size:x}", h.finish()))
    }

    /// The cache location to pass to `BestSource` for `video` (creates the directory).
    pub fn cache_for(&self, video: &Path) -> io::Result<IndexCache> {
        fs::create_dir_all(&self.dir)?;
        Ok(IndexCache::File(self.dir.join(Self::key_for(video)?)))
    }

    /// Index files belonging to `video` (one per indexed track).
    pub fn entries_for(&self, video: &Path) -> io::Result<Vec<PathBuf>> {
        let key = Self::key_for(video)?;
        Ok(self
            .list()?
            .into_iter()
            .filter(|(p, _, _)| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&key) && n.ends_with(SUFFIX))
            })
            .map(|(p, _, _)| p)
            .collect())
    }

    /// Whether some track of `video` has a cached index.
    #[must_use]
    pub fn has_index(&self, video: &Path) -> bool {
        self.entries_for(video).is_ok_and(|e| !e.is_empty())
    }

    /// Marks `video`'s entries as recently used.
    pub fn touch(&self, video: &Path) -> io::Result<()> {
        let now = SystemTime::now();
        for p in self.entries_for(video)? {
            File::options().write(true).open(&p)?.set_modified(now)?;
        }
        Ok(())
    }

    /// Deletes all cached `.bsindex` files belonging to `video` (e.g., before rebuilding after
    /// detecting corruption). Returns the number of files deleted.
    pub fn remove_entries_for(&self, video: &Path) -> io::Result<usize> {
        let mut removed = 0;
        for p in self.entries_for(video)? {
            if fs::remove_file(&p).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Deletes least recently used entries until the store fits in its limit.
    /// Returns the number of bytes removed.
    pub fn enforce_limit(&self) -> io::Result<u64> {
        self.enforce_limit_preserving(None)
    }

    /// Deletes least recently used entries until the store fits in `max_bytes`, keeping `keep`'s
    /// entries out of eviction when non-`None`. Returns the number of bytes removed.
    pub fn enforce_limit_preserving(&self, keep: Option<&Path>) -> io::Result<u64> {
        let keep_key = keep.and_then(|v| Self::key_for(v).ok());
        let mut entries = self.list()?;
        let mut total: u64 = entries.iter().map(|(_, len, _)| len).sum();
        entries.sort_by_key(|(_, _, t)| *t);
        let mut removed = 0;
        for (p, len, _) in entries {
            if total <= self.max_bytes {
                break;
            }
            let protected = keep_key.as_deref().is_some_and(|key| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(key))
            });
            if protected {
                continue;
            }
            if fs::remove_file(&p).is_ok() {
                total = total.saturating_sub(len);
                removed += len;
            }
        }
        Ok(removed)
    }

    fn list(&self) -> io::Result<Vec<(PathBuf, u64, SystemTime)>> {
        let rd = match fs::read_dir(&self.dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            if !p.to_string_lossy().ends_with(SUFFIX) {
                continue;
            }
            if let Ok(m) = e.metadata() {
                out.push((p, m.len(), m.modified().unwrap_or(UNIX_EPOCH)));
            }
        }
        Ok(out)
    }
}

fn read_up_to(f: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// FNV-1a 64: stable across runs and builds (unlike `DefaultHasher`).
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, data: &[u8]) {
        for &b in data {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hlouis-store-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn key_changes_with_path_size_mtime_and_header_bytes() {
        let d = tmp("key");
        let f = d.join("a.mkv");
        fs::write(&f, vec![1u8; 200_000]).unwrap();
        let k1 = IndexStore::key_for(&f).unwrap();
        assert_eq!(k1, IndexStore::key_for(&f).unwrap());

        // 1. Header/content byte change (with restored mtime so content hash itself is tested).
        let orig_mtime = fs::metadata(&f).unwrap().modified().unwrap();
        let mut data = vec![1u8; 200_000];
        data[16] = 2;
        fs::write(&f, &data).unwrap();
        File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(orig_mtime)
            .unwrap();
        let k_hdr = IndexStore::key_for(&f).unwrap();
        assert_ne!(k1, k_hdr);

        // 2. Mtime change alone.
        File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(orig_mtime - std::time::Duration::from_secs(60))
            .unwrap();
        assert_ne!(k_hdr, IndexStore::key_for(&f).unwrap());

        // 3. Size change.
        data.push(1);
        fs::write(&f, &data).unwrap();
        let k_size = IndexStore::key_for(&f).unwrap();
        assert_ne!(k_hdr, k_size);

        // 4. Path change (identical bytes & mtime at sibling path).
        let f2 = d.join("b.mkv");
        fs::write(&f2, &data).unwrap();
        let m2 = fs::metadata(&f).unwrap().modified().unwrap();
        File::options()
            .write(true)
            .open(&f2)
            .unwrap()
            .set_modified(m2)
            .unwrap();
        assert_ne!(k_size, IndexStore::key_for(&f2).unwrap());
    }

    #[test]
    fn lru_cleanup_preserves_active_video_when_requested() {
        let d = tmp("lru_preserve");
        let store = IndexStore::new(d.clone(), 150);
        let active = d.join("active.mkv");
        fs::write(&active, vec![7u8; 1024]).unwrap();
        let active_key = IndexStore::key_for(&active).unwrap();
        let active_idx = d.join(format!("{active_key}.0.bsindex"));
        let other_idx = d.join("other.0.bsindex");
        fs::write(&active_idx, vec![0u8; 100]).unwrap();
        fs::write(&other_idx, vec![0u8; 100]).unwrap();
        let now = SystemTime::now();
        File::options()
            .write(true)
            .open(&active_idx)
            .unwrap()
            .set_modified(now - std::time::Duration::from_secs(500))
            .unwrap();
        File::options()
            .write(true)
            .open(&other_idx)
            .unwrap()
            .set_modified(now)
            .unwrap();

        // Even though active_idx is older, preserving `active` evicts `other` instead.
        assert_eq!(store.enforce_limit_preserving(Some(&active)).unwrap(), 100);
        assert!(active_idx.exists());
        assert!(!other_idx.exists());
        assert!(store.remove_entries_for(&active).unwrap() >= 1);
        assert!(!active_idx.exists());
    }

    #[test]
    fn lru_cleanup_removes_oldest_first() {
        let d = tmp("lru");
        let store = IndexStore::new(d.clone(), 250);
        let now = SystemTime::now();
        for (i, name) in ["old", "mid", "new"].iter().enumerate() {
            let p = d.join(format!("{name}.0.bsindex"));
            fs::write(&p, vec![0u8; 100]).unwrap();
            let age = std::time::Duration::from_secs(1000 * (3 - i as u64));
            File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(now - age)
                .unwrap();
        }
        assert_eq!(store.enforce_limit().unwrap(), 100);
        assert!(!d.join("old.0.bsindex").exists());
        assert!(d.join("mid.0.bsindex").exists());
        assert!(d.join("new.0.bsindex").exists());
    }
}
