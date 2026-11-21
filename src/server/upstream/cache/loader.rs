use super::*;
use std::time::Instant;

#[derive(Debug, Default, PartialEq, Eq)]
pub(in crate::server) struct CacheLoaderStats {
    pub(in crate::server) scanned: u64,
    pub(in crate::server) loaded: u64,
    pub(in crate::server) loaded_bytes: u64,
    pub(in crate::server) removed: u64,
    pub(in crate::server) freed: u64,
}

impl CacheLoaderStats {
    pub(super) fn merge(&mut self, other: Self) {
        self.scanned = self.scanned.saturating_add(other.scanned);
        self.loaded = self.loaded.saturating_add(other.loaded);
        self.loaded_bytes = self.loaded_bytes.saturating_add(other.loaded_bytes);
        self.removed = self.removed.saturating_add(other.removed);
        self.freed = self.freed.saturating_add(other.freed);
    }

    pub(in crate::server) fn is_empty(&self) -> bool {
        self.scanned == 0 && self.removed == 0
    }
}

pub(super) async fn load_zone(zone: &ProxyCacheZone) -> io::Result<CacheLoaderStats> {
    if !zone.begin_load() {
        return Ok(CacheLoaderStats::default());
    }
    let result = load_cold_zone(zone).await;
    zone.finish_load(result.is_ok());
    result
}

async fn load_cold_zone(zone: &ProxyCacheZone) -> io::Result<CacheLoaderStats> {
    if !zone.config.path.exists() {
        return Ok(CacheLoaderStats::default());
    }
    let stale_temporary = remove_stale_temporary_files(&zone.config.path, "cache").await?;
    let files = fs::scan_files(&zone.config.path).await?;
    let mut stats = CacheLoaderStats {
        scanned: files.len() as u64,
        removed: stale_temporary,
        ..CacheLoaderStats::default()
    };
    let mut batch = 0_usize;
    let mut batch_started = Instant::now();

    for file in files {
        let hash = key::cache_file_hash(&file.path);
        if let Some(hash) = &hash
            && entry::is_cache_entry(&file.path).await?
            && zone.load_index(hash)
        {
            stats.loaded = stats.loaded.saturating_add(1);
            stats.loaded_bytes = stats.loaded_bytes.saturating_add(file.length);
        } else {
            match tokio::fs::remove_file(&file.path).await {
                Ok(()) => {
                    stats.removed = stats.removed.saturating_add(1);
                    stats.freed = stats.freed.saturating_add(file.length);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        batch = batch.saturating_add(1);
        if batch >= zone.config.loader_files.max(1)
            || batch_started.elapsed() >= zone.config.loader_threshold
        {
            if !zone.config.loader_sleep.is_zero() {
                tokio::time::sleep(zone.config.loader_sleep).await;
            }
            batch = 0;
            batch_started = Instant::now();
        }
    }
    Ok(stats)
}
