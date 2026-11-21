use super::*;
use std::sync::RwLock as StdRwLock;

pub(in crate::server) struct ProxyCacheRegistry {
    zones: StdRwLock<HashMap<String, Arc<ProxyCacheZone>>>,
}

impl Default for ProxyCacheRegistry {
    fn default() -> Self {
        Self {
            zones: StdRwLock::new(HashMap::new()),
        }
    }
}

impl ProxyCacheRegistry {
    pub(in crate::server) fn new(http: Option<&HttpConfig>) -> Self {
        let registry = Self::default();
        registry.reload(http);
        registry
    }

    pub(in crate::server) fn reload(&self, http: Option<&HttpConfig>) {
        let mut current = self.zones.write().expect("proxy cache registry lock");
        let mut zones = HashMap::new();
        if let Some(http) = http {
            for config in http.cache_paths.values() {
                let zone = current
                    .get(&config.keys_zone)
                    .filter(|zone| zone.matches_config(config))
                    .cloned()
                    .unwrap_or_else(|| Arc::new(ProxyCacheZone::new((**config).clone())));
                zones.insert(config.keys_zone.clone(), zone);
            }
        }
        *current = zones;
    }

    pub(in crate::server) fn zone(&self, name: &str) -> Option<Arc<ProxyCacheZone>> {
        self.zones
            .read()
            .expect("proxy cache registry lock")
            .get(name)
            .cloned()
    }

    fn zones(&self) -> Vec<Arc<ProxyCacheZone>> {
        self.zones
            .read()
            .expect("proxy cache registry lock")
            .values()
            .cloned()
            .collect()
    }

    pub(in crate::server) async fn load(&self) -> io::Result<CacheLoaderStats> {
        let mut total = CacheLoaderStats::default();
        let mut first_error = None;
        for zone in self.zones() {
            match loader::load_zone(&zone).await {
                Ok(stats) => total.merge(stats),
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(total),
        }
    }

    pub(in crate::server) async fn maintain(&self) -> io::Result<CacheManagerStats> {
        let mut total = CacheManagerStats::default();
        for zone in self.zones() {
            total.merge(manager::maintain_zone(&zone).await?);
        }
        Ok(total)
    }

    pub(in crate::server) async fn manage_loop(
        &self,
        logger: Arc<Logger>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = interval.tick() => {
                    match self.maintain().await {
                        Ok(stats) if stats.removed != 0 => {
                            logger.log(
                                LogLevel::Debug,
                                format!(
                                    "cache manager removed {} files ({} bytes)",
                                    stats.removed, stats.freed
                                ),
                            );
                        }
                        Ok(_) => {}
                        Err(error) => {
                            logger.log(
                                LogLevel::Error,
                                format!("cache manager failed: {error}"),
                            );
                        }
                    }
                }
            }
        }
    }

    pub(in crate::server) async fn loader_loop(
        &self,
        logger: Arc<Logger>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = interval.tick() => self.load_and_log(&logger).await,
            }
        }
    }

    async fn load_and_log(&self, logger: &Logger) {
        match self.load().await {
            Ok(stats) if !stats.is_empty() => logger.log(
                LogLevel::Notice,
                format!(
                    "cache loader scanned {} files, loaded {} ({} bytes), removed {} ({} bytes)",
                    stats.scanned, stats.loaded, stats.loaded_bytes, stats.removed, stats.freed
                ),
            ),
            Ok(_) => {}
            Err(error) => logger.log(LogLevel::Error, format!("cache loader failed: {error}")),
        }
    }
}
