use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use flate2::Compression;
use flate2::write::GzEncoder;

use crate::config::{AccessLogBufferConfig, OpenLogFileCacheConfig};

use super::file_cache::LogFileCaches;

const FLUSH_TICK: Duration = Duration::from_millis(50);

static ACCESS_LOGS: OnceLock<Mutex<AccessLogs>> = OnceLock::new();
static FLUSH_THREAD: Once = Once::new();

struct AccessLogs {
    buffered: HashMap<PathBuf, BufferedLog>,
    file_caches: LogFileCaches,
}

struct BufferedLog {
    config: AccessLogBufferConfig,
    file_cache: Option<Arc<OpenLogFileCacheConfig>>,
    bytes: Vec<u8>,
    flush_at: Option<Instant>,
}

pub fn append_access_log(
    path: Option<&Path>,
    line: &str,
    buffer: Option<&AccessLogBufferConfig>,
    file_cache: Option<&Arc<OpenLogFileCacheConfig>>,
) {
    let Some(path) = path else {
        return;
    };
    let mut logs = access_logs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let AccessLogs {
        buffered: buffered_logs,
        file_caches,
    } = &mut *logs;
    if let Some(config) = buffer {
        let buffered = buffered_logs
            .entry(path.to_path_buf())
            .or_insert_with(|| BufferedLog::new(config.clone(), file_cache.cloned()));
        if buffered.config != *config || !same_file_cache(&buffered.file_cache, file_cache) {
            buffered.flush(path, file_caches);
            buffered.config = config.clone();
            buffered.file_cache = file_cache.cloned();
        }
        buffered.write(path, line, file_caches);
    } else if let Some(buffered) = buffered_logs.get_mut(path) {
        buffered.write(path, line, file_caches);
    } else {
        file_caches.write(path, line.as_bytes(), file_cache);
    }
}

pub fn flush_access_logs() {
    let mut logs = access_logs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let AccessLogs {
        buffered,
        file_caches,
    } = &mut *logs;
    for (path, buffered) in buffered {
        buffered.flush(path, file_caches);
    }
}

pub fn reopen_access_logs() {
    flush_access_logs();
    let mut logs = access_logs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    logs.file_caches.clear();
}

impl BufferedLog {
    fn new(config: AccessLogBufferConfig, file_cache: Option<Arc<OpenLogFileCacheConfig>>) -> Self {
        Self {
            config,
            file_cache,
            bytes: Vec::new(),
            flush_at: None,
        }
    }

    fn write(&mut self, path: &Path, line: &str, files: &mut LogFileCaches) {
        let now = Instant::now();
        if self.flush_at.is_some_and(|flush_at| now >= flush_at) {
            self.flush(path, files);
        }
        if line.len() > self.config.size {
            self.flush(path, files);
            write_payload(
                path,
                line.as_bytes(),
                self.config.gzip_level,
                self.file_cache.as_ref(),
                files,
            );
            return;
        }
        if self.bytes.len() + line.len() > self.config.size {
            self.flush(path, files);
        }
        let was_empty = self.bytes.is_empty();
        self.bytes.extend_from_slice(line.as_bytes());
        if was_empty {
            self.flush_at = self.config.flush.map(|flush| now + flush);
        }
    }

    fn flush_if_due(&mut self, path: &Path, now: Instant, files: &mut LogFileCaches) {
        if self.flush_at.is_some_and(|flush_at| now >= flush_at) {
            self.flush(path, files);
        }
    }

    fn flush(&mut self, path: &Path, files: &mut LogFileCaches) {
        self.flush_at = None;
        if self.bytes.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.bytes);
        write_payload(
            path,
            &bytes,
            self.config.gzip_level,
            self.file_cache.as_ref(),
            files,
        );
    }
}

fn write_payload(
    path: &Path,
    bytes: &[u8],
    gzip_level: Option<u32>,
    file_cache: Option<&Arc<OpenLogFileCacheConfig>>,
    files: &mut LogFileCaches,
) {
    let payload = match gzip_level {
        Some(level) => {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::new(level));
            if encoder.write_all(bytes).is_err() {
                return;
            }
            let Ok(payload) = encoder.finish() else {
                return;
            };
            payload
        }
        None => bytes.to_vec(),
    };
    files.write(path, &payload, file_cache);
}

fn access_logs() -> &'static Mutex<AccessLogs> {
    let logs = ACCESS_LOGS.get_or_init(|| {
        Mutex::new(AccessLogs {
            buffered: HashMap::new(),
            file_caches: LogFileCaches::new(),
        })
    });
    FLUSH_THREAD.call_once(|| {
        let _ = thread::Builder::new()
            .name("starling-access-log-flush".into())
            .spawn(flush_loop);
    });
    logs
}

fn flush_loop() {
    loop {
        thread::sleep(FLUSH_TICK);
        let now = Instant::now();
        let mut logs = ACCESS_LOGS
            .get()
            .expect("access log state initialized before flush thread")
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let AccessLogs {
            buffered,
            file_caches,
        } = &mut *logs;
        for (path, buffered) in buffered {
            buffered.flush_if_due(path, now, file_caches);
        }
    }
}

fn same_file_cache(
    left: &Option<Arc<OpenLogFileCacheConfig>>,
    right: Option<&Arc<OpenLogFileCacheConfig>>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => Arc::ptr_eq(left, right),
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
    }
}
