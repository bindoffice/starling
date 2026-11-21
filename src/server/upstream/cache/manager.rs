use super::*;
use std::collections::HashSet;

mod disk;

const CACHE_MANAGER_FILES_DEFAULT: usize = 100;

#[derive(Debug, Default, PartialEq, Eq)]
pub(in crate::server) struct CacheManagerStats {
    pub(in crate::server) scanned: u64,
    pub(in crate::server) removed: u64,
    pub(in crate::server) freed: u64,
}

impl CacheManagerStats {
    pub(super) fn merge(&mut self, other: Self) {
        self.scanned = self.scanned.saturating_add(other.scanned);
        self.removed = self.removed.saturating_add(other.removed);
        self.freed = self.freed.saturating_add(other.freed);
    }
}

pub(super) async fn maintain_zone(zone: &ProxyCacheZone) -> io::Result<CacheManagerStats> {
    if !zone.config.path.exists() {
        return Ok(CacheManagerStats::default());
    }
    let mut files = Vec::new();
    for file in fs::scan_files(&zone.config.path).await? {
        if entry::is_cache_entry(&file.path).await? {
            files.push(file);
        }
    }
    let mut stats = CacheManagerStats {
        scanned: files.len() as u64,
        ..CacheManagerStats::default()
    };
    let limit = zone
        .config
        .manager_files
        .max(1)
        .min(CACHE_MANAGER_FILES_DEFAULT.saturating_mul(1000));
    let mut removed = HashSet::new();

    for file in &files {
        if stats.removed as usize >= limit {
            break;
        }
        let inactive = file
            .modified
            .elapsed()
            .is_ok_and(|elapsed| elapsed >= zone.config.inactive);
        if inactive {
            remove_file(zone, file, &mut stats, &mut removed).await?;
        }
    }

    let total = files
        .iter()
        .filter(|file| !removed.contains(&file.path))
        .map(|file| file.length)
        .sum::<u64>();
    let mut required = zone
        .config
        .max_size
        .map(|max_size| max_size_eviction(total, max_size))
        .unwrap_or_default();
    if let Some(min_free) = zone.config.min_free
        && let Some(available) = disk::available_space(&zone.config.path)?
    {
        required = required.max(min_free_eviction(available, min_free));
    }
    if required == 0 {
        return Ok(stats);
    };
    let mut remaining = files
        .iter()
        .filter(|file| !removed.contains(&file.path))
        .collect::<Vec<_>>();
    remaining.sort_by_key(|file| file.modified);
    let mut freed = 0_u64;
    for file in remaining {
        if freed >= required || stats.removed as usize >= limit {
            break;
        }
        if let Some(bytes) = remove_file(zone, file, &mut stats, &mut removed).await? {
            freed = freed.saturating_add(bytes);
        }
    }
    Ok(stats)
}

fn max_size_eviction(total: u64, max_size: u64) -> u64 {
    if total < max_size {
        0
    } else {
        total.saturating_sub(max_size).saturating_add(1)
    }
}

fn min_free_eviction(available: u64, min_free: u64) -> u64 {
    if available > min_free {
        0
    } else {
        min_free.saturating_sub(available).saturating_add(1)
    }
}

async fn remove_file(
    zone: &ProxyCacheZone,
    file: &fs::CacheFile,
    stats: &mut CacheManagerStats,
    removed: &mut HashSet<PathBuf>,
) -> io::Result<Option<u64>> {
    match tokio::fs::remove_file(&file.path).await {
        Ok(()) => {
            if let Some(hash) = key::cache_file_hash(&file.path) {
                zone.forget_hash(&hash);
            }
            removed.insert(file.path.clone());
            stats.removed = stats.removed.saturating_add(1);
            stats.freed = stats.freed.saturating_add(file.length);
            if !zone.config.manager_sleep.is_zero() {
                tokio::time::sleep(zone.config.manager_sleep).await;
            }
            Ok(Some(file.length))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(hash) = key::cache_file_hash(&file.path) {
                zone.forget_hash(&hash);
            }
            removed.insert(file.path.clone());
            Ok(Some(file.length))
        }
        Err(error) => Err(error),
    }
}
