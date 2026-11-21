use super::*;

pub(in crate::server) enum ReusableUpstreamConnection {
    Http1 {
        upstream: String,
        config: UpstreamKeepaliveConfig,
        connection: UpstreamConnection,
    },
    Http2 {
        upstream: String,
        config: UpstreamKeepaliveConfig,
        connection: ProxyV2Connection,
    },
}

pub(in crate::server) struct ProxyAttemptResult {
    pub(in crate::server) response: Response,
    pub(in crate::server) reusable: Option<ReusableUpstreamConnection>,
    pub(in crate::server) timing: UpstreamAttemptTiming,
}

pub(in crate::server) struct ProxyAttemptTarget<'a> {
    pub(super) host: &'a str,
    pub(super) port: u16,
    pub(super) keepalive: Option<&'a UpstreamKeepaliveContext>,
    pub(super) revalidation: Option<&'a CacheRevalidation>,
    pub(super) streaming_allowed: bool,
}
