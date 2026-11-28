use super::*;

pub(in crate::server) enum ReusableUpstreamConnection {
    Http1 {
        upstream: Arc<str>,
        config: UpstreamKeepaliveConfig,
        connection: UpstreamConnection,
    },
    Http2 {
        upstream: Arc<str>,
        config: UpstreamKeepaliveConfig,
        connection: ProxyV2Connection,
    },
}

pub(in crate::server) struct ProxyAttemptResult {
    pub(in crate::server) response: Response,
    pub(in crate::server) reusable: Option<ReusableUpstreamConnection>,
    pub(in crate::server) upgrade: Option<ProxyUpgrade>,
    pub(in crate::server) timing: UpstreamAttemptTiming,
}

pub(in crate::server) struct ProxyUpgrade {
    pub(in crate::server) connection: UpstreamConnection,
    pub(in crate::server) read_timeout: Duration,
    pub(in crate::server) write_timeout: Duration,
    pub(in crate::server) buffer_size: usize,
}

pub(in crate::server) struct ProxyAttemptTarget<'a> {
    pub(super) host: &'a str,
    pub(super) port: u16,
    pub(super) keepalive: Option<&'a UpstreamKeepaliveContext>,
    pub(super) revalidation: Option<&'a CacheRevalidation>,
    pub(super) streaming_allowed: bool,
    pub(super) strip_cache_headers: bool,
}
