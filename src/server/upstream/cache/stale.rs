use super::*;
use crate::config::ProxyCacheUseStaleConfig;
use store::duration_millis;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::server) struct CacheStaleWindows {
    pub(super) updating: Duration,
    pub(super) error: Duration,
}

impl CacheStaleWindows {
    pub(super) fn from_millis(updating: u64, error: u64) -> Self {
        Self {
            updating: Duration::from_millis(updating),
            error: Duration::from_millis(error),
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(in crate::server) struct CacheStaleState {
    pub(in crate::server) updating: bool,
    pub(in crate::server) error: bool,
}

impl CacheStaleState {
    pub(super) fn at(expires: u64, windows: CacheStaleWindows, now: u64) -> Self {
        Self {
            updating: expires.saturating_add(duration_millis(windows.updating)) >= now,
            error: expires.saturating_add(duration_millis(windows.error)) >= now,
        }
    }
}

pub(in crate::server) struct CacheValidity {
    pub(in crate::server) valid: Duration,
    pub(super) stale: CacheStaleWindows,
}

impl CacheValidity {
    pub(super) fn new(valid: Duration, stale: CacheStaleWindows) -> Self {
        Self { valid, stale }
    }
}

pub(in crate::server) fn allows_stale_updating(
    use_stale: &ProxyCacheUseStaleConfig,
    cached: &CachedResponse,
) -> bool {
    use_stale.updating() || cached.stale.updating
}

pub(in crate::server) fn allows_stale_error(
    use_stale: &ProxyCacheUseStaleConfig,
    cached: &CachedResponse,
    failure: &ProxyFailure,
) -> bool {
    cached.stale.error || proxy_cache_use_stale_on_failure(use_stale, failure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_windows_are_measured_from_expiration() {
        let windows = CacheStaleWindows::from_millis(30_000, 60_000);
        let state = CacheStaleState::at(100_000, windows, 129_000);
        assert!(state.updating);
        assert!(state.error);

        let state = CacheStaleState::at(100_000, windows, 131_000);
        assert!(!state.updating);
        assert!(state.error);

        let state = CacheStaleState::at(100_000, windows, 161_000);
        assert!(!state.updating);
        assert!(!state.error);

        let windows = CacheStaleWindows::from_millis(1_500, 2_500);
        let state = CacheStaleState::at(100_000, windows, 101_400);
        assert!(state.updating);
        assert!(state.error);
        let state = CacheStaleState::at(100_000, windows, 101_600);
        assert!(!state.updating);
        assert!(state.error);
    }
}
