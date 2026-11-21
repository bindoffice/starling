use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::types::{
    LeastTimeConfig, LeastTimeMode, UpstreamAttemptTiming, UpstreamHealth, UpstreamServer,
    UpstreamTiming,
};

#[derive(Debug)]
pub struct UpstreamPeerGuard {
    pub(in crate::config::compile) active: Arc<AtomicUsize>,
    pub(in crate::config::compile) health: Arc<Mutex<UpstreamHealth>>,
    pub(in crate::config::compile) timing: Arc<Mutex<UpstreamTiming>>,
    pub(in crate::config::compile) failed: bool,
}

impl Drop for UpstreamPeerGuard {
    fn drop(&mut self) {
        let mut health = self
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.failed {
            health.failures = health.failures.saturating_add(1);
            health.checked = Some(Instant::now());
        } else {
            health.failures = 0;
        }
        let mut timing = self
            .timing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        update_inflight(&mut timing, -1);
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}

impl UpstreamServer {
    pub fn try_acquire(&self) -> Option<UpstreamPeerGuard> {
        self.try_acquire_with_health(true)
    }

    pub fn try_acquire_with_health(&self, check_health: bool) -> Option<UpstreamPeerGuard> {
        if !self.has_capacity() || (check_health && !self.health_available()) {
            return None;
        }
        loop {
            let active = self.active.load(Ordering::Relaxed);
            if self.max_conns != 0 && active >= self.max_conns {
                return None;
            }
            if self
                .active
                .compare_exchange_weak(
                    active,
                    active.saturating_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return Some(UpstreamPeerGuard {
                    active: self.active.clone(),
                    health: self.health.clone(),
                    timing: {
                        let mut timing = self
                            .timing
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        update_inflight(&mut timing, 1);
                        self.timing.clone()
                    },
                    failed: false,
                });
            }
        }
    }

    pub fn active_counter(&self) -> Arc<AtomicUsize> {
        self.active.clone()
    }

    pub fn active_connections(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    pub fn is_available(&self) -> bool {
        self.has_capacity() && self.health_available()
    }

    pub fn has_capacity(&self) -> bool {
        if self.down {
            return false;
        }
        if self.max_conns != 0 && self.active_connections() >= self.max_conns {
            return false;
        }
        true
    }

    pub fn health_available(&self) -> bool {
        if self.max_fails == 0 {
            return true;
        }
        let health = self
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        health.failures < self.max_fails
            || health.checked.is_none_or(|checked| {
                Instant::now().saturating_duration_since(checked) > self.fail_timeout
            })
    }

    pub(in crate::config::compile) fn least_time_eta(&self, config: LeastTimeConfig) -> u64 {
        let timing = self
            .timing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut response_time = match config.mode {
            LeastTimeMode::Header => timing.header_time,
            LeastTimeMode::Connect => timing.connect_time,
            LeastTimeMode::FirstByte => timing.first_byte_time,
            LeastTimeMode::LastByte => timing.response_time,
        };
        if config.inflight && timing.inflight_requests != 0 {
            response_time = response_time.max(current_inflight_time(&timing));
        }
        let response_time = if response_time > 5_000 {
            5_000
        } else {
            response_time + 20 - response_time % 20
        };
        response_time.saturating_mul(self.active_connections() as u64 + 1)
    }
}

impl UpstreamPeerGuard {
    pub fn mark_failed(&mut self) {
        self.failed = true;
    }

    pub fn record_timing(&mut self, timing: UpstreamAttemptTiming) {
        let mut stats = self
            .timing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        update_average(&mut stats.connect_time, timing.connect);
        if let Some(first_byte) = timing.first_byte {
            update_average(&mut stats.first_byte_time, first_byte);
        }
        update_average(&mut stats.header_time, timing.header);
        update_average(&mut stats.response_time, timing.response);
    }
}

pub(in crate::config::compile) fn update_average(current: &mut u64, sample: Duration) {
    let sample = sample.as_millis().min(u128::from(u64::MAX)) as u64;
    *current = if *current == 0 {
        sample
    } else {
        (0.5 + sample as f64 * 0.05 + *current as f64 * 0.95) as u64
    };
}

pub(in crate::config::compile) fn update_inflight(timing: &mut UpstreamTiming, change: isize) {
    let now = Instant::now();
    if timing.inflight_requests != 0 {
        let elapsed = now
            .saturating_duration_since(timing.inflight_started)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        timing.inflight_time = timing
            .inflight_time
            .saturating_add(elapsed.saturating_mul(timing.inflight_requests as u64));
    }
    timing.inflight_started = now;
    if change < 0 {
        timing.inflight_requests = timing.inflight_requests.saturating_sub(1);
    } else {
        timing.inflight_requests = timing.inflight_requests.saturating_add(change as usize);
    }
}

fn current_inflight_time(timing: &UpstreamTiming) -> u64 {
    if timing.inflight_requests == 0 {
        return 0;
    }
    let elapsed = Instant::now()
        .saturating_duration_since(timing.inflight_started)
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    let total = timing
        .inflight_time
        .saturating_add(elapsed.saturating_mul(timing.inflight_requests as u64));
    total / timing.inflight_requests as u64
}
