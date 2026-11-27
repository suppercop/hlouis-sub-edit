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

    /// `%LOCALAPPDATA%\hlouis-sub-edit\bsindex` with the default limit.
    #[must_use]
    pub fn default_location() -> Option<Self> {
        let base = std::env::var_os("LOCALAPPDATA")?;
        Some(Self::new(
            PathBuf::from(base).join("hlouis-sub-edit").join("bsindex"),
            Self::DEFAULT_MAX_BYTES,
        ))
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

    /// Deletes least recently used entries until the store fits in its limit.
    /// Returns the number of bytes removed.
    pub fn enforce_limit(&self) -> io::Result<u64> {
        let mut entries = self.list()?;
        let mut total: u64 = entries.iter().map(|(_, len, _)| len).sum();
        entries.sort_by_key(|(_, _, t)| *t);
        let mut removed = 0;
        for (p, len, _) in entries {
            if total <= self.max_bytes {
                break;
            }
            if fs::remove_file(&p).is_ok() {
                total -= len;
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
    fn key_changes_with_content_and_is_stable() {
        let d = tmp("key");
        let f = d.join("a.mkv");
        fs::write(&f, vec![1u8; 200_000]).unwrap();
        let k1 = IndexStore::key_for(&f).unwrap();
        assert_eq!(k1, IndexStore::key_for(&f).unwrap());
        let mut data = vec![1u8; 200_000];
        data[100_000] = 2;
        fs::write(&f, data).unwrap();
        assert_ne!(k1, IndexStore::key_for(&f).unwrap());
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
