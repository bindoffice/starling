use std::collections::HashMap;
use std::fs::{File, Metadata, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::OpenLogFileCacheConfig;

pub(super) struct LogFileCaches {
    caches: HashMap<usize, OpenLogFileCache>,
}

impl LogFileCaches {
    pub(super) fn new() -> Self {
        Self {
            caches: HashMap::new(),
        }
    }

    pub(super) fn write(
        &mut self,
        path: &Path,
        bytes: &[u8],
        config: Option<&Arc<OpenLogFileCacheConfig>>,
    ) {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && std::fs::create_dir_all(parent).is_err()
        {
            return;
        }
        let Some(config) = config else {
            write_direct(path, bytes);
            return;
        };
        let id = Arc::as_ptr(config) as usize;
        let cache = self
            .caches
            .entry(id)
            .or_insert_with(|| OpenLogFileCache::new(config));
        if !cache.matches(config) {
            *cache = OpenLogFileCache::new(config);
        }
        cache.write(path, bytes);
    }

    pub(super) fn clear(&mut self) {
        self.caches.clear();
    }
}

struct OpenLogFileCache {
    max: usize,
    inactive: Duration,
    min_uses: usize,
    valid: Duration,
    files: HashMap<PathBuf, CachedLogFile>,
}

impl OpenLogFileCache {
    fn new(config: &OpenLogFileCacheConfig) -> Self {
        Self {
            max: config.max,
            inactive: config.inactive,
            min_uses: config.min_uses,
            valid: config.valid,
            files: HashMap::new(),
        }
    }

    fn matches(&self, config: &OpenLogFileCacheConfig) -> bool {
        self.max == config.max
            && self.inactive == config.inactive
            && self.min_uses == config.min_uses
            && self.valid == config.valid
    }

    fn write(&mut self, path: &Path, bytes: &[u8]) {
        let now = Instant::now();
        self.expire_inactive(now);

        if let Some(file) = self.files.get_mut(path) {
            file.uses = file.uses.saturating_add(1);
            file.last_used = now;
            if self.valid.is_zero() || now.duration_since(file.last_validated) >= self.valid {
                if !file.revalidate(path) {
                    self.files.remove(path);
                } else {
                    file.last_validated = now;
                }
            }
        }

        if !self.files.contains_key(path) {
            self.make_room();
            let Some((file, identity)) = open_log_file(path) else {
                return;
            };
            self.files.insert(
                path.to_path_buf(),
                CachedLogFile {
                    file: Some(file),
                    identity,
                    last_used: now,
                    last_validated: now,
                    uses: 1,
                },
            );
        }

        let Some(file) = self.files.get_mut(path) else {
            return;
        };
        if file.file.is_none() {
            let Some((handle, identity)) = open_log_file(path) else {
                return;
            };
            file.file = Some(handle);
            file.identity = identity;
            file.last_validated = now;
        }
        let Some(handle) = file.file.as_mut() else {
            return;
        };
        if handle.write_all(bytes).is_err() {
            file.file = None;
            return;
        }
        if file.uses < self.min_uses as u64 {
            file.file = None;
        }
    }

    fn expire_inactive(&mut self, now: Instant) {
        let inactive = self.inactive;
        self.files
            .retain(|_, file| now.duration_since(file.last_used) <= inactive);
    }

    fn make_room(&mut self) {
        while self.files.len() >= self.max {
            let oldest = self
                .files
                .iter()
                .min_by_key(|(_, file)| file.last_used)
                .map(|(path, _)| path.clone());
            let Some(oldest) = oldest else {
                break;
            };
            self.files.remove(&oldest);
        }
    }
}

struct CachedLogFile {
    file: Option<File>,
    identity: FileIdentity,
    last_used: Instant,
    last_validated: Instant,
    uses: u64,
}

impl CachedLogFile {
    fn revalidate(&mut self, path: &Path) -> bool {
        let Ok(metadata) = std::fs::metadata(path) else {
            return false;
        };
        file_identity(&metadata) == self.identity
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
fn file_identity(metadata: &Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;

    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(not(unix))]
fn file_identity(metadata: &Metadata) -> FileIdentity {
    FileIdentity {
        device: metadata.len(),
        inode: metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos() as u64),
    }
}

fn open_log_file(path: &Path) -> Option<(File, FileIdentity)> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    let identity = file_identity(&file.metadata().ok()?);
    Some((file, identity))
}

fn write_direct(path: &Path, bytes: &[u8]) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "starling-log-file-cache-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn cache_config(max: usize, min_uses: usize, valid: Duration) -> Arc<OpenLogFileCacheConfig> {
        Arc::new(OpenLogFileCacheConfig {
            max,
            inactive: Duration::from_secs(10),
            min_uses,
            valid,
        })
    }

    #[test]
    fn revalidates_replaced_log_file_before_append() {
        let dir = temp_dir();
        let path = dir.join("access.log");
        let rotated = dir.join("access.log.old");
        let config = cache_config(4, 1, Duration::ZERO);
        let mut caches = LogFileCaches::new();

        caches.write(&path, b"first\n", Some(&config));
        std::fs::rename(&path, &rotated).unwrap();
        caches.write(&path, b"second\n", Some(&config));

        assert_eq!(std::fs::read_to_string(&rotated).unwrap(), "first\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second\n");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn enforces_max_cache_entries() {
        let dir = temp_dir();
        let config = cache_config(2, 1, Duration::from_secs(60));
        let mut caches = LogFileCaches::new();
        for name in ["first.log", "second.log", "third.log"] {
            caches.write(&dir.join(name), b"line\n", Some(&config));
        }

        let cache = caches.caches.get(&(Arc::as_ptr(&config) as usize)).unwrap();
        assert_eq!(cache.files.len(), 2);
        assert!(!cache.files.contains_key(&dir.join("first.log")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retains_file_after_min_uses_threshold() {
        let dir = temp_dir();
        let path = dir.join("access.log");
        let config = cache_config(4, 2, Duration::from_secs(60));
        let mut caches = LogFileCaches::new();

        caches.write(&path, b"first\n", Some(&config));
        let cache = caches.caches.get(&(Arc::as_ptr(&config) as usize)).unwrap();
        assert!(cache.files[&path].file.is_none());

        caches.write(&path, b"second\n", Some(&config));
        let cache = caches.caches.get(&(Arc::as_ptr(&config) as usize)).unwrap();
        assert!(cache.files[&path].file.is_some());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "first\nsecond\n");
        let _ = std::fs::remove_dir_all(dir);
    }
}
