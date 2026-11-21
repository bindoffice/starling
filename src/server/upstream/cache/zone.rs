use super::*;
use crate::config::{ProxyCacheLockConfig, ProxyCachePathConfig};
use std::sync::atomic::{AtomicU8, Ordering};

const LOAD_COLD: u8 = 0;
const LOAD_RUNNING: u8 = 1;
const LOAD_WARM: u8 = 2;

pub(in crate::server) struct ProxyCacheZone {
    pub(super) config: ProxyCachePathConfig,
    index: Mutex<index::CacheIndex>,
    errors: Mutex<HashMap<String, CachedError>>,
    locks: lock::CacheLocks,
    load_state: AtomicU8,
}

pub(in crate::server) enum CacheLookup {
    Miss,
    Expired(CachedResponse),
    Hit(CachedResponse),
    Error(u16),
}

pub(in crate::server) struct CachedResponse {
    pub(in crate::server) status: u16,
    pub(in crate::server) headers: Vec<Header>,
    pub(in crate::server) created: u64,
    pub(in crate::server) stale: CacheStaleState,
    pub(in crate::server) stale_windows: CacheStaleWindows,
    pub(in crate::server) last_modified: Option<u64>,
    pub(in crate::server) etag: Option<String>,
    pub(in crate::server) body: ResponseBody,
}

struct CachedError {
    status: u16,
    expires: u64,
}

impl ProxyCacheZone {
    pub(super) fn new(config: ProxyCachePathConfig) -> Self {
        let index = index::CacheIndex::new(config.keys_zone_size);
        Self {
            config,
            index: Mutex::new(index),
            errors: Mutex::new(HashMap::new()),
            locks: lock::CacheLocks::default(),
            load_state: AtomicU8::new(LOAD_COLD),
        }
    }

    pub(super) fn matches_config(&self, config: &ProxyCachePathConfig) -> bool {
        self.config == *config
    }

    pub(super) fn begin_load(&self) -> bool {
        self.load_state
            .compare_exchange(LOAD_COLD, LOAD_RUNNING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(super) fn finish_load(&self, success: bool) {
        self.load_state.store(
            if success { LOAD_WARM } else { LOAD_COLD },
            Ordering::Release,
        );
    }

    pub(in crate::server) fn entry_path(&self, cache_key: &str) -> PathBuf {
        key::cache_entry_path(
            &self.config.path,
            self.config.levels,
            &key::cache_key_hash(cache_key),
        )
    }

    pub(in crate::server) async fn lookup(&self, cache_key: &str) -> io::Result<CacheLookup> {
        if let Some(status) = self.lookup_error(cache_key) {
            return Ok(CacheLookup::Error(status));
        }
        let path = self.entry_path(cache_key);
        let Some(entry) = entry::read_entry(&path).await? else {
            return Ok(CacheLookup::Miss);
        };
        let now = store::unix_time_millis();
        let expired = entry.expires <= now;
        let stale = CacheStaleState::at(entry.expires, entry.stale, now);
        if let ResponseBody::File { file, .. } = &entry.body {
            let _ = file.set_modified(SystemTime::now()).await;
        }
        let cached = CachedResponse {
            status: entry.status,
            headers: entry.headers,
            created: entry.created,
            stale,
            stale_windows: entry.stale,
            last_modified: entry.last_modified,
            etag: entry.etag,
            body: entry.body,
        };
        Ok(if expired {
            CacheLookup::Expired(cached)
        } else {
            CacheLookup::Hit(cached)
        })
    }

    pub(in crate::server) async fn acquire_lock(
        &self,
        cache_key: &str,
        config: &ProxyCacheLockConfig,
    ) -> CacheLockOutcome {
        self.locks.acquire(cache_key, config).await
    }

    pub(in crate::server) async fn try_acquire_update_lock(
        &self,
        cache_key: &str,
    ) -> Option<CacheLockGuard> {
        let config = ProxyCacheLockConfig {
            enabled: true,
            timeout: Duration::ZERO,
            age: Duration::from_secs(5),
        };
        match self.locks.acquire(cache_key, &config).await {
            CacheLockOutcome::Acquired(guard) => Some(guard),
            CacheLockOutcome::TimedOut => None,
        }
    }

    pub(in crate::server) fn is_updating(&self, cache_key: &str) -> bool {
        self.locks.is_held(cache_key)
    }

    pub(in crate::server) fn note_use(&self, cache_key: &str, min_uses: usize) -> bool {
        let hash = key::cache_key_hash(cache_key);
        self.index
            .lock()
            .expect("proxy cache index lock")
            .note_use(&hash, min_uses)
    }

    pub(in crate::server) fn store_error(
        &self,
        cache_key: &str,
        status: u16,
        validity: &CacheValidity,
    ) {
        let now = store::unix_time_millis();
        let expires = now.saturating_add(store::duration_millis(validity.valid));
        let hash = key::cache_key_hash(cache_key);
        let mut errors = self.errors.lock().expect("proxy cache error lock");
        errors.retain(|_, error| error.expires > now);
        errors.insert(hash, CachedError { status, expires });
    }

    fn lookup_error(&self, cache_key: &str) -> Option<u16> {
        let now = store::unix_time_millis();
        let hash = key::cache_key_hash(cache_key);
        let mut errors = self.errors.lock().expect("proxy cache error lock");
        if errors.get(&hash).is_some_and(|error| error.expires <= now) {
            errors.remove(&hash);
        }
        errors
            .get(&hash)
            .filter(|error| error.expires > now)
            .map(|error| error.status)
    }

    fn clear_error(&self, cache_key: &str) {
        let hash = key::cache_key_hash(cache_key);
        self.errors
            .lock()
            .expect("proxy cache error lock")
            .remove(&hash);
    }

    pub(super) fn load_index(&self, hash: &str) -> bool {
        self.index
            .lock()
            .expect("proxy cache index lock")
            .load(hash)
    }

    pub(super) fn forget_hash(&self, hash: &str) {
        self.index
            .lock()
            .expect("proxy cache index lock")
            .remove(hash);
        self.errors
            .lock()
            .expect("proxy cache error lock")
            .remove(hash);
    }

    pub(in crate::server) async fn store(
        &self,
        cache_key: &str,
        response: &Response,
        validity: &CacheValidity,
    ) -> io::Result<()> {
        self.clear_error(cache_key);
        store::store_response(self, cache_key, response, validity).await
    }

    pub(in crate::server) async fn store_or_tee(
        &self,
        cache_key: &str,
        response: Response,
        validity: &CacheValidity,
        guard: Option<CacheLockGuard>,
    ) -> Response {
        self.clear_error(cache_key);
        if response.body.is_stream() {
            tee::attach_tee(self, cache_key, response, validity, guard)
        } else {
            let _ = self.store(cache_key, &response, validity).await;
            response
        }
    }

    pub(in crate::server) async fn refresh(
        &self,
        cache_key: &str,
        expected_created: u64,
        validity: &CacheValidity,
    ) -> io::Result<bool> {
        let created = store::unix_time_millis();
        let expires = created.saturating_add(store::duration_millis(validity.valid));
        entry::update_validity(
            &self.entry_path(cache_key),
            expected_created,
            created,
            expires,
            validity.stale,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone() -> ProxyCacheZone {
        zone_at(PathBuf::from("/tmp/starling-cache-error-test"))
    }

    fn zone_at(path: PathBuf) -> ProxyCacheZone {
        ProxyCacheZone::new(ProxyCachePathConfig {
            path,
            levels: [0; 3],
            keys_zone: "error_cache".into(),
            keys_zone_size: 1024,
            inactive: Duration::from_secs(60),
            max_size: None,
            use_temp_path: false,
            loader_files: 1,
            loader_sleep: Duration::ZERO,
            loader_threshold: Duration::ZERO,
            manager_files: 1,
            manager_sleep: Duration::ZERO,
            manager_threshold: Duration::ZERO,
            min_free: None,
        })
    }

    #[tokio::test]
    async fn caches_negative_status_until_configured_expiry() {
        let zone = zone();
        let expired = CacheValidity::new(Duration::ZERO, CacheStaleWindows::default());
        zone.store_error("GET /item", 502, &expired);
        assert_eq!(zone.lookup_error("GET /item"), None);

        let valid = CacheValidity::new(Duration::from_secs(60), CacheStaleWindows::default());
        zone.store_error("GET /item", 504, &valid);
        assert_eq!(zone.lookup_error("GET /item"), Some(504));
        assert!(matches!(
            zone.lookup("GET /item").await,
            Ok(CacheLookup::Error(504))
        ));

        zone.clear_error("GET /item");
        assert_eq!(zone.lookup_error("GET /item"), None);
    }

    #[tokio::test]
    async fn error_node_takes_precedence_over_an_expired_entry_until_its_validity_expires() {
        let root =
            std::env::temp_dir().join(format!("starling-cache-error-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let zone = zone_at(root.join("cache"));
        let cached = Response::bytes(200, "text/plain", b"old".to_vec());
        let cached_validity = CacheValidity::new(
            Duration::from_millis(1),
            CacheStaleWindows::from_millis(0, 0),
        );
        zone.store("GET /item", &cached, &cached_validity)
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(matches!(
            zone.lookup("GET /item").await.unwrap(),
            CacheLookup::Expired(_)
        ));

        let error_validity = CacheValidity::new(
            Duration::from_millis(20),
            CacheStaleWindows::from_millis(0, 0),
        );
        zone.store_error("GET /item", 502, &error_validity);
        assert!(matches!(
            zone.lookup("GET /item").await.unwrap(),
            CacheLookup::Error(502)
        ));

        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(matches!(
            zone.lookup("GET /item").await.unwrap(),
            CacheLookup::Expired(_)
        ));

        drop(zone);
        let _ = std::fs::remove_dir_all(root);
    }
}
