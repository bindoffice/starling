use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct UpstreamConfig {
    pub name: Arc<str>,
    pub servers: Vec<UpstreamServer>,
    pub method: UpstreamMethod,
    pub hash_key: Option<String>,
    pub keepalive: Option<UpstreamKeepaliveConfig>,
    pub next: Arc<AtomicUsize>,
}

#[derive(Debug, Clone)]
pub struct UpstreamKeepaliveConfig {
    pub max_cached: usize,
    pub requests: u64,
    pub time: Duration,
    pub timeout: Duration,
    pub local: bool,
}

impl Default for UpstreamKeepaliveConfig {
    fn default() -> Self {
        Self {
            max_cached: 32,
            requests: 1000,
            time: Duration::from_secs(3600),
            timeout: Duration::from_secs(60),
            local: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamMethod {
    RoundRobin,
    LeastConn,
    LeastTime(LeastTimeConfig),
    IpHash,
    Random { two: bool },
    Hash { consistent: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeastTimeConfig {
    pub mode: LeastTimeMode,
    pub inflight: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeastTimeMode {
    Header,
    LastByte,
    Connect,
    FirstByte,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct UpstreamAttemptTiming {
    pub connect: Duration,
    pub first_byte: Option<Duration>,
    pub header: Duration,
    pub response: Duration,
}

impl UpstreamAttemptTiming {
    pub fn response_only(response: Duration) -> Self {
        Self {
            connect: Duration::ZERO,
            first_byte: None,
            header: response,
            response,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpstreamServer {
    pub address: String,
    pub weight: usize,
    pub max_conns: usize,
    pub max_fails: usize,
    pub fail_timeout: Duration,
    pub backup: bool,
    pub down: bool,
    pub(in crate::config::compile) active: Arc<AtomicUsize>,
    pub(in crate::config::compile) health: Arc<Mutex<UpstreamHealth>>,
    pub(in crate::config::compile) timing: Arc<Mutex<UpstreamTiming>>,
}

#[derive(Debug)]
pub(in crate::config::compile) struct UpstreamHealth {
    pub(in crate::config::compile) failures: usize,
    pub(in crate::config::compile) checked: Option<Instant>,
}

#[derive(Debug)]
pub(in crate::config::compile) struct UpstreamTiming {
    pub(in crate::config::compile) connect_time: u64,
    pub(in crate::config::compile) first_byte_time: u64,
    pub(in crate::config::compile) header_time: u64,
    pub(in crate::config::compile) response_time: u64,
    pub(in crate::config::compile) inflight_time: u64,
    pub(in crate::config::compile) inflight_started: Instant,
    pub(in crate::config::compile) inflight_requests: usize,
}

impl Default for UpstreamTiming {
    fn default() -> Self {
        Self {
            connect_time: 0,
            first_byte_time: 0,
            header_time: 0,
            response_time: 0,
            inflight_time: 0,
            inflight_started: Instant::now(),
            inflight_requests: 0,
        }
    }
}
