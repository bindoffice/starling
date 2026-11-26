use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime};

const MAX_OCSP_CACHE_ENTRIES: usize = 4096;

#[derive(Debug)]
pub(super) struct OcspCache {
    entries: Mutex<VecDeque<CacheEntry>>,
}

#[derive(Debug)]
struct CacheEntry {
    key: Vec<u8>,
    expires: SystemTime,
}

impl OcspCache {
    pub(super) fn from_spec(spec: Option<&str>) -> Option<Arc<Self>> {
        let name = spec?.strip_prefix("shared:")?.split(':').next()?;
        if name.is_empty() {
            return None;
        }
        static CACHES: OnceLock<Mutex<HashMap<String, Weak<OcspCache>>>> = OnceLock::new();
        let caches = CACHES.get_or_init(|| Mutex::new(HashMap::new()));
        let mut caches = caches
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cache) = caches.get(name).and_then(Weak::upgrade) {
            return Some(cache);
        }
        let cache = Arc::new(Self {
            entries: Mutex::new(VecDeque::new()),
        });
        caches.insert(name.to_string(), Arc::downgrade(&cache));
        Some(cache)
    }

    pub(super) fn contains(&self, key: &[u8]) -> bool {
        let now = SystemTime::now();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries.retain(|entry| entry.expires > now);
        entries.iter().any(|entry| entry.key == key)
    }

    pub(super) fn insert(&self, key: Vec<u8>, next_update: Option<SystemTime>) {
        let now = SystemTime::now();
        let expires = next_update.unwrap_or_else(|| now + Duration::from_secs(3600));
        if expires <= now {
            return;
        }
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries.retain(|entry| entry.expires > now && entry.key != key);
        if entries.len() >= MAX_OCSP_CACHE_ENTRIES {
            entries.pop_front();
        }
        entries.push_back(CacheEntry { key, expires });
    }
}
