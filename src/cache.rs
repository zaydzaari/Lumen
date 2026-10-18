//! Size-capped disk caches (album art, lyrics) and helpers for librespot's audio
//! cache directory.
//!
//! The audio cache itself is librespot's own encrypted format, managed and evicted
//! by librespot (`FsSizeLimiter`). Lumen only measures and clears that directory;
//! it never exports or decrypts audio.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use sha2::{Digest as _, Sha256};

/// A directory of small files with a total size cap. Least-recently-used files
/// (by modification time, refreshed on read) are evicted first.
pub struct DiskCache {
    dir: PathBuf,
    max_bytes: u64,
    /// Approximate current size; `None` until first scanned.
    approx: Mutex<Option<u64>>,
}

impl DiskCache {
    pub fn new(dir: impl Into<PathBuf>, max_bytes: u64) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self {
            dir,
            max_bytes,
            approx: Mutex::new(None),
        })
    }

    /// Map an arbitrary key (URL, "artist|title") to a safe file name.
    pub fn key_to_name(key: &str, ext: &str) -> String {
        let digest = Sha256::digest(key.as_bytes());
        let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
        format!("{hex}.{ext}")
    }

    fn path(&self, key: &str, ext: &str) -> PathBuf {
        self.dir.join(Self::key_to_name(key, ext))
    }

    /// Read an entry and mark it as recently used.
    pub fn get(&self, key: &str, ext: &str) -> Option<Vec<u8>> {
        let path = self.path(key, ext);
        let data = fs::read(&path).ok()?;
        touch(&path);
        Some(data)
    }

    pub fn contains(&self, key: &str, ext: &str) -> bool {
        self.path(key, ext).exists()
    }

    /// Age of an entry, if present.
    pub fn age(&self, key: &str, ext: &str) -> Option<std::time::Duration> {
        let meta = fs::metadata(self.path(key, ext)).ok()?;
        SystemTime::now().duration_since(meta.modified().ok()?).ok()
    }

    pub fn remove(&self, key: &str, ext: &str) {
        let _ = fs::remove_file(self.path(key, ext));
    }

    /// Store an entry atomically, then evict if over the cap.
    pub fn put(&self, key: &str, ext: &str, data: &[u8]) -> Result<()> {
        let path = self.path(key, ext);
        let tmp = path.with_extension(format!("{ext}.tmp"));
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(data)?;
        }
        fs::rename(&tmp, &path)?;
        let over = {
            let mut approx = self
                .approx
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let current = match *approx {
                Some(v) => v + data.len() as u64,
                None => dir_size(&self.dir),
            };
            *approx = Some(current);
            current > self.max_bytes
        };
        if over {
            self.evict()?;
        }
        Ok(())
    }

    /// Delete least-recently-used files until the cache is at or below 90% of the cap.
    /// Returns the number of bytes removed.
    pub fn evict(&self) -> Result<u64> {
        let mut entries: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        let mut total = 0u64;
        for entry in fs::read_dir(&self.dir)?.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            total += meta.len();
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            entries.push((mtime, meta.len(), entry.path()));
        }
        let mut removed = 0u64;
        if total > self.max_bytes {
            let target = self.max_bytes / 10 * 9;
            entries.sort_by_key(|e| e.0);
            for (_, len, path) in entries {
                if total <= target {
                    break;
                }
                if fs::remove_file(&path).is_ok() {
                    total = total.saturating_sub(len);
                    removed += len;
                }
            }
        }
        *self
            .approx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(total);
        Ok(removed)
    }

    #[cfg(test)]
    pub fn size(&self) -> u64 {
        dir_size(&self.dir)
    }

    pub fn clear(&self) -> Result<()> {
        clear_dir(&self.dir)?;
        *self
            .approx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(0);
        Ok(())
    }
}

fn touch(path: &Path) {
    if let Ok(f) = fs::File::options().append(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}

/// Total size of all files below `dir` (recursive). Missing dir → 0.
pub fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    for entry in rd.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

/// Remove everything inside `dir` but keep the directory itself.
pub fn clear_dir(dir: &Path) -> Result<()> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let res = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        if let Err(e) = res {
            tracing::warn!("could not remove {}: {e}", path.display());
        }
    }
    Ok(())
}

/// Human-readable byte size.
pub fn fmt_bytes(b: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if b >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", b as f64 / (MB * 1024.0))
    } else if b >= 1024 * 1024 {
        format!("{:.1} MB", b as f64 / MB)
    } else {
        format!("{:.0} KB", b as f64 / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn set_age(cache: &DiskCache, key: &str, secs_ago: u64) {
        let p = cache.path(key, "bin");
        let f = fs::File::options().append(true).open(p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(secs_ago))
            .unwrap();
    }

    #[test]
    fn put_get_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path(), 1024 * 1024).unwrap();
        c.put("https://i.scdn.co/image/x", "bin", b"hello").unwrap();
        assert_eq!(
            c.get("https://i.scdn.co/image/x", "bin").as_deref(),
            Some(&b"hello"[..])
        );
        assert!(c.get("missing", "bin").is_none());
        assert!(c.contains("https://i.scdn.co/image/x", "bin"));
        c.remove("https://i.scdn.co/image/x", "bin");
        assert!(!c.contains("https://i.scdn.co/image/x", "bin"));
    }

    #[test]
    fn keys_are_safe_file_names() {
        let n = DiskCache::key_to_name("../../etc/passwd", "txt");
        assert!(!n.contains('/') && !n.contains('\\') && !n.contains(".."));
        assert!(n.ends_with(".txt"));
        assert_ne!(n, DiskCache::key_to_name("other", "txt"));
    }

    #[test]
    fn eviction_removes_least_recently_used_first() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path(), 3000).unwrap();
        let blob = vec![0u8; 1000];
        c.put("a", "bin", &blob).unwrap();
        c.put("b", "bin", &blob).unwrap();
        c.put("c", "bin", &blob).unwrap();
        set_age(&c, "a", 300);
        set_age(&c, "b", 100);
        set_age(&c, "c", 200);
        // Reading "a" refreshes it, so "c" becomes the oldest.
        assert!(c.get("a", "bin").is_some());
        // Fourth entry pushes us over 3000 bytes → evict down to <= 2700.
        c.put("d", "bin", &blob).unwrap();
        assert!(c.size() <= 2700, "size {}", c.size());
        assert!(c.contains("a", "bin"), "recently read entry must survive");
        assert!(c.contains("d", "bin"), "newest entry must survive");
        assert!(!c.contains("c", "bin"), "oldest entry must be evicted");
    }

    #[test]
    fn cache_never_exceeds_cap_under_many_writes() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path(), 10_000).unwrap();
        for i in 0..100 {
            c.put(&format!("k{i}"), "bin", &[1u8; 700]).unwrap();
            assert!(c.size() <= 10_000 + 700);
        }
        assert!(c.size() <= 10_000);
    }

    #[test]
    fn clear_and_dir_size() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("x"), [0u8; 10]).unwrap();
        fs::write(dir.path().join("y"), [0u8; 5]).unwrap();
        assert_eq!(dir_size(dir.path()), 15);
        clear_dir(dir.path()).unwrap();
        assert_eq!(dir_size(dir.path()), 0);
        assert!(dir.path().exists());
        assert_eq!(dir_size(&dir.path().join("nope")), 0);
    }

    #[test]
    fn byte_formatting() {
        assert_eq!(fmt_bytes(2048), "2 KB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(fmt_bytes(3 * 1024 * 1024 * 1024), "3.00 GB");
    }
}
