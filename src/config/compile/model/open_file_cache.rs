use std::time::Duration;

#[derive(Debug, Clone)]
pub struct OpenFileCacheConfig {
    pub enabled: bool,
    pub max: usize,
    pub inactive: Duration,
    pub min_uses: usize,
    pub valid: Duration,
    pub errors: bool,
    pub events: bool,
}

impl Default for OpenFileCacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max: 0,
            inactive: Duration::from_secs(60),
            min_uses: 1,
            valid: Duration::from_secs(60),
            errors: false,
            events: false,
        }
    }
}
