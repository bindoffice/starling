use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::OpenFileCacheConfig;

thread_local! {
    static CACHES: RefCell<HashMap<usize, OpenFileCache>> = RefCell::new(HashMap::new());
}

const MAX_CACHED_FILE_SIZE: u64 = 64 * 1024;
const MAX_CACHE_CONFIGS: usize = 32;

pub(super) enum CachedOpenFile {
    Memory(Vec<u8>),
    File(File),
}

pub(super) fn open_cached(
    path: &Path,
    config: &Arc<OpenFileCacheConfig>,
) -> io::Result<(CachedOpenFile, Metadata)> {
    if !config.enabled {
        return open_uncached(path);
    }
    CACHES.with(|caches| {
        let mut caches = caches.borrow_mut();
        let key = Arc::as_ptr(config) as usize;
        if !caches.contains_key(&key) && caches.len() >= MAX_CACHE_CONFIGS {
            let oldest = caches
                .iter()
                .min_by_key(|(_, cache)| cache.last_used)
                .map(|(key, _)| *key);
            if let Some(oldest) = oldest {
                caches.remove(&oldest);
            }
        }
        let cache = caches
            .entry(key)
            .or_insert_with(|| OpenFileCache::new(config));
        if !cache.matches(config) {
            *cache = OpenFileCache::new(config);
        }
        cache.open(path)
    })
}

fn open_uncached(path: &Path) -> io::Result<(CachedOpenFile, Metadata)> {
    let file = open_static_file(path)?;
    let metadata = file.metadata()?;
    if metadata.len() <= MAX_CACHED_FILE_SIZE {
        return Ok((
            CachedOpenFile::Memory(read_small_file(file, metadata.len())?),
            metadata,
        ));
    }
    Ok((CachedOpenFile::File(file), metadata))
}

fn open_static_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options.open(path)
}

struct OpenFileCache {
    last_used: Instant,
    max: usize,
    inactive: Duration,
    min_uses: usize,
    valid: Duration,
    files: HashMap<PathBuf, CachedFile>,
}

impl OpenFileCache {
    fn new(config: &OpenFileCacheConfig) -> Self {
        Self {
            last_used: Instant::now(),
            max: config.max,
            inactive: config.inactive,
            min_uses: config.min_uses,
            valid: config.valid,
            files: HashMap::new(),
        }
    }

    fn matches(&self, config: &OpenFileCacheConfig) -> bool {
        self.max == config.max
            && self.inactive == config.inactive
            && self.min_uses == config.min_uses
            && self.valid == config.valid
    }

    fn open(&mut self, path: &Path) -> io::Result<(CachedOpenFile, Metadata)> {
        let now = Instant::now();
        self.last_used = now;
        self.expire_inactive(now);

        if let Some(cached) = self.files.get_mut(path) {
            cached.uses = cached.uses.saturating_add(1);
            cached.last_used = now;
            if self.valid.is_zero() || now.duration_since(cached.last_validated) >= self.valid {
                if !cached.revalidate(path)? {
                    self.files.remove(path);
                    return open_uncached(path);
                }
                cached.last_validated = now;
            }
            return Ok((
                CachedOpenFile::Memory(cached.body.clone()),
                cached.metadata.clone(),
            ));
        }

        let file = open_static_file(path)?;
        let metadata = file.metadata()?;
        if metadata.len() > MAX_CACHED_FILE_SIZE {
            return Ok((CachedOpenFile::File(file), metadata));
        }
        let body = read_small_file(file, metadata.len())?;
        self.make_room();
        self.files.insert(
            path.to_path_buf(),
            CachedFile {
                body: body.clone(),
                identity: FileIdentity::from_metadata(&metadata),
                metadata: metadata.clone(),
                last_used: now,
                last_validated: now,
                uses: 1,
            },
        );
        Ok((CachedOpenFile::Memory(body), metadata))
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

fn read_small_file(file: File, length: u64) -> io::Result<Vec<u8>> {
    let mut body = Vec::with_capacity(length as usize);
    file.take(length).read_to_end(&mut body)?;
    Ok(body)
}

struct CachedFile {
    body: Vec<u8>,
    identity: FileIdentity,
    metadata: Metadata,
    last_used: Instant,
    last_validated: Instant,
    uses: u64,
}

impl CachedFile {
    fn revalidate(&mut self, path: &Path) -> io::Result<bool> {
        let metadata = std::fs::metadata(path)?;
        let identity = FileIdentity::from_metadata(&metadata);
        if metadata.len() > MAX_CACHED_FILE_SIZE {
            return Ok(false);
        }
        if identity != self.identity {
            self.body = read_small_file(open_static_file(path)?, metadata.len())?;
            self.identity = identity;
        }
        self.metadata = metadata;
        Ok(true)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    size: u64,
    modified_seconds: u64,
    modified_nanos: u32,
}

impl FileIdentity {
    #[cfg(unix)]
    fn from_metadata(metadata: &Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;

        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.size(),
            modified_seconds: metadata.mtime() as u64,
            modified_nanos: metadata.mtime_nsec() as u32,
        }
    }

    #[cfg(not(unix))]
    fn from_metadata(metadata: &Metadata) -> Self {
        let (modified_seconds, modified_nanos) = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| (duration.as_secs(), duration.subsec_nanos()))
            .unwrap_or_default();
        Self {
            device: 0,
            inode: 0,
            size: metadata.len(),
            modified_seconds,
            modified_nanos,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_file(contents: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "starling-open-file-cache-{}-{id}.txt",
            std::process::id()
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn config(valid: Duration) -> Arc<OpenFileCacheConfig> {
        Arc::new(OpenFileCacheConfig {
            enabled: true,
            max: 4,
            inactive: Duration::from_secs(60),
            min_uses: 1,
            valid,
            errors: false,
            events: false,
        })
    }

    fn read_body(file: CachedOpenFile) -> Vec<u8> {
        match file {
            CachedOpenFile::Memory(body) => body,
            CachedOpenFile::File(mut file) => {
                let mut body = Vec::new();
                file.read_to_end(&mut body).unwrap();
                body
            }
        }
    }

    #[test]
    fn serves_the_cached_file_until_the_valid_interval_expires() {
        let path = temp_file("first");
        let config = config(Duration::from_secs(60));
        let (first, _) = open_cached(&path, &config).unwrap();
        assert_eq!(read_body(first), b"first");

        std::fs::write(&path, "second").unwrap();
        let (second, metadata) = open_cached(&path, &config).unwrap();
        assert_eq!(read_body(second), b"first");
        assert_eq!(metadata.len(), 5);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn zero_valid_reopens_a_replaced_file() {
        let path = temp_file("first");
        let config = config(Duration::ZERO);
        let _ = open_cached(&path, &config).unwrap();
        std::fs::write(&path, "second").unwrap();

        let (file, metadata) = open_cached(&path, &config).unwrap();
        assert_eq!(read_body(file), b"second");
        assert_eq!(metadata.len(), 6);

        let _ = std::fs::remove_file(path);
    }
}
