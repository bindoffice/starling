use super::*;

mod accel;
mod body_copy;
mod body_window;
mod cache;
mod file_ops;
mod socket;
mod store_stream;
mod streaming;
mod streaming_prefetch;
mod tee;
pub(super) use body_copy::copy_response_body;
pub(super) use cache::*;
pub(super) use file_ops::*;
use socket::*;
pub(in crate::server) use socket::{UpstreamSocketOptions, evaluate_upstream_bind};
pub(super) use store_stream::*;
pub(super) use streaming::*;
pub(super) use tee::*;

pub(in crate::server) use accel::*;

#[derive(Debug)]
pub(super) enum ProxyFailure {
    Failure,
    Timeout,
    InvalidHeader,
    RequestBody(u16),
}

pub(super) trait UpstreamIo: AsyncRead + AsyncWrite + Unpin + Send {
    fn idle_is_open(&mut self) -> bool;
}

pub(super) fn tcp_idle_is_open(stream: &TcpStream) -> bool {
    let mut byte = [0_u8; 1];
    matches!(stream.try_read(&mut byte), Err(err) if err.kind() == io::ErrorKind::WouldBlock)
}

impl UpstreamIo for TcpStream {
    fn idle_is_open(&mut self) -> bool {
        tcp_idle_is_open(self)
    }
}

impl UpstreamIo for Box<dyn UpstreamIo> {
    fn idle_is_open(&mut self) -> bool {
        self.as_mut().idle_is_open()
    }
}

impl<T> UpstreamIo for tokio_rustls::client::TlsStream<T>
where
    T: UpstreamIo,
{
    fn idle_is_open(&mut self) -> bool {
        self.get_mut().0.idle_is_open()
    }
}

#[cfg(unix)]
pub(super) fn unix_idle_is_open(stream: &tokio::net::UnixStream) -> bool {
    let mut byte = [0_u8; 1];
    matches!(stream.try_read(&mut byte), Err(err) if err.kind() == io::ErrorKind::WouldBlock)
}

#[cfg(unix)]
impl UpstreamIo for tokio::net::UnixStream {
    fn idle_is_open(&mut self) -> bool {
        unix_idle_is_open(self)
    }
}

#[cfg(test)]
impl UpstreamIo for tokio::io::DuplexStream {
    fn idle_is_open(&mut self) -> bool {
        true
    }
}

pub(super) type BoxedUpstreamIo = Box<dyn UpstreamIo>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UpstreamPoolKey {
    pub(super) generation: u64,
    pub(super) scheme: String,
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) tls_name: Option<String>,
    pub(super) tls_verify: bool,
    pub(super) tls_trusted_certificate: Option<PathBuf>,
    pub(super) tls_verify_depth: Option<u32>,
    pub(super) tls_crl: Option<PathBuf>,
    pub(super) tls_client_certificate: Option<PathBuf>,
    pub(super) tls_client_certificate_key: Option<PathBuf>,
    pub(super) tls_protocols: Option<TlsProtocols>,
    pub(super) tls_ciphers: Option<TlsCipherConfig>,
    pub(super) proxy_bind: Option<String>,
    pub(super) socket_keepalive: bool,
    pub(super) socket_rcvbuf: usize,
    pub(super) socket_sndbuf: usize,
    pub(super) http_version: String,
}

pub(super) struct UpstreamConnection {
    pub(super) reader: BufReader<BoxedUpstreamIo>,
    pub(super) key: UpstreamPoolKey,
    pub(super) created: Instant,
    pub(super) idle_since: Instant,
    pub(super) requests: u64,
}

impl UpstreamConnection {
    pub(super) fn new(io: BoxedUpstreamIo, key: UpstreamPoolKey) -> Self {
        let now = Instant::now();
        Self {
            reader: BufReader::new(io),
            key,
            created: now,
            idle_since: now,
            requests: 0,
        }
    }

    pub(super) fn idle_is_open(&mut self) -> bool {
        self.reader.buffer().is_empty() && self.reader.get_mut().idle_is_open()
    }
}

pub(super) enum PooledUpstreamConnection {
    Http1(UpstreamConnection),
    Http2(ProxyV2Connection),
}

impl PooledUpstreamConnection {
    fn key(&self) -> &UpstreamPoolKey {
        match self {
            Self::Http1(connection) => &connection.key,
            Self::Http2(connection) => connection.key(),
        }
    }

    fn created(&self) -> Instant {
        match self {
            Self::Http1(connection) => connection.created,
            Self::Http2(connection) => connection.created(),
        }
    }

    fn idle_since(&self) -> Instant {
        match self {
            Self::Http1(connection) => connection.idle_since,
            Self::Http2(connection) => connection.idle_since(),
        }
    }

    fn requests(&self) -> u64 {
        match self {
            Self::Http1(connection) => connection.requests,
            Self::Http2(connection) => connection.requests(),
        }
    }

    fn idle_is_open(&mut self) -> bool {
        match self {
            Self::Http1(connection) => connection.idle_is_open(),
            Self::Http2(connection) => connection.idle_is_open(),
        }
    }

    fn mark_idle(&mut self, idle_since: Instant) {
        match self {
            Self::Http1(connection) => connection.idle_since = idle_since,
            Self::Http2(connection) => connection.mark_idle(idle_since),
        }
    }
}

impl From<UpstreamConnection> for PooledUpstreamConnection {
    fn from(connection: UpstreamConnection) -> Self {
        Self::Http1(connection)
    }
}

#[derive(Default)]
pub(super) struct UpstreamKeepalivePool {
    pub(super) generation: AtomicU64,
    pub(super) entries: tokio::sync::Mutex<HashMap<String, VecDeque<PooledUpstreamConnection>>>,
}

impl UpstreamKeepalivePool {
    pub(super) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub(super) async fn take(
        &self,
        upstream: &str,
        key: &UpstreamPoolKey,
        config: &UpstreamKeepaliveConfig,
    ) -> Option<UpstreamConnection> {
        match self.take_pooled(upstream, key, config).await? {
            PooledUpstreamConnection::Http1(connection) => Some(connection),
            PooledUpstreamConnection::Http2(_) => None,
        }
    }

    pub(super) async fn take_http2(
        &self,
        upstream: &str,
        key: &UpstreamPoolKey,
        config: &UpstreamKeepaliveConfig,
    ) -> Option<ProxyV2Connection> {
        match self.take_pooled(upstream, key, config).await? {
            PooledUpstreamConnection::Http2(connection) => Some(connection),
            PooledUpstreamConnection::Http1(_) => None,
        }
    }

    async fn take_pooled(
        &self,
        upstream: &str,
        key: &UpstreamPoolKey,
        config: &UpstreamKeepaliveConfig,
    ) -> Option<PooledUpstreamConnection> {
        if config.max_cached == 0 {
            return None;
        }
        let mut pools = self.entries.lock().await;
        let entries = pools.get_mut(upstream)?;
        let now = Instant::now();
        let generation = self.generation();
        let mut index = 0;
        while index < entries.len() {
            let entry = &entries[index];
            let expired = now.saturating_duration_since(entry.idle_since()) >= config.timeout;
            let over_limit = entry.requests() >= config.requests;
            let lifetime_expired = now.saturating_duration_since(entry.created()) >= config.time;
            let stale_generation = entry.key().generation != generation;
            if expired || over_limit || lifetime_expired || stale_generation {
                entries.remove(index);
                continue;
            }
            if entries[index].key() == key {
                if !entries[index].idle_is_open() {
                    entries.remove(index);
                    continue;
                }
                return entries.remove(index);
            }
            index += 1;
        }
        None
    }

    pub(super) async fn put(
        &self,
        upstream: &str,
        config: &UpstreamKeepaliveConfig,
        connection: UpstreamConnection,
    ) {
        self.put_pooled(upstream, config, connection.into()).await;
    }

    pub(super) async fn put_http2(
        &self,
        upstream: &str,
        config: &UpstreamKeepaliveConfig,
        connection: ProxyV2Connection,
    ) {
        self.put_pooled(
            upstream,
            config,
            PooledUpstreamConnection::Http2(connection),
        )
        .await;
    }

    async fn put_pooled(
        &self,
        upstream: &str,
        config: &UpstreamKeepaliveConfig,
        mut connection: PooledUpstreamConnection,
    ) {
        if config.max_cached == 0
            || config.requests == 0
            || connection.key().generation != self.generation()
            || connection.requests() >= config.requests
            || connection.created().elapsed() >= config.time
            || !connection.idle_is_open()
        {
            return;
        }
        let mut pools = self.entries.lock().await;
        if connection.key().generation != self.generation() {
            return;
        }
        let entries = pools.entry(upstream.to_string()).or_default();
        let now = Instant::now();
        entries.retain(|entry| {
            now.saturating_duration_since(entry.idle_since()) < config.timeout
                && now.saturating_duration_since(entry.created()) < config.time
        });
        while entries.len() >= config.max_cached {
            entries.pop_back();
        }
        connection.mark_idle(now);
        entries.push_front(connection);
    }

    pub(super) async fn clear(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.entries.lock().await.clear();
    }

    #[cfg(test)]
    pub(super) async fn len(&self, upstream: &str) -> usize {
        self.entries
            .lock()
            .await
            .get(upstream)
            .map_or(0, VecDeque::len)
    }
}

pub(super) fn proxy_failure_from_io(err: io::Error) -> ProxyFailure {
    if let Some(status) = crate::http::request_body_error_status(&err) {
        return ProxyFailure::RequestBody(status);
    }
    match err.kind() {
        io::ErrorKind::TimedOut => ProxyFailure::Timeout,
        io::ErrorKind::InvalidData => ProxyFailure::InvalidHeader,
        _ => ProxyFailure::Failure,
    }
}

pub(super) struct NextUpstreamState<'a> {
    config: &'a NextUpstreamConfig,
    started: Instant,
    attempts: usize,
}

impl<'a> NextUpstreamState<'a> {
    pub(super) fn new(config: &'a NextUpstreamConfig) -> Self {
        Self {
            config,
            started: Instant::now(),
            attempts: 0,
        }
    }

    pub(super) fn start_attempt(&mut self) -> bool {
        if self.config.tries != 0 && self.attempts >= self.config.tries {
            return false;
        }
        self.attempts += 1;
        true
    }

    pub(super) fn can_retry_failure(&self, request: &Request, failure: &ProxyFailure) -> bool {
        if !self.has_attempt_capacity() || !self.within_timeout() || !self.allows_method(request) {
            return false;
        }
        match failure {
            ProxyFailure::Failure => self.config.retry_error(),
            ProxyFailure::Timeout => self.config.retry_timeout(),
            ProxyFailure::InvalidHeader => self.config.retry_invalid_header(),
            ProxyFailure::RequestBody(_) => false,
        }
    }

    pub(super) fn can_retry_status(&self, request: &Request, status: u16) -> bool {
        self.has_attempt_capacity()
            && self.within_timeout()
            && self.allows_method(request)
            && self.config.retry_status(status)
    }

    fn has_attempt_capacity(&self) -> bool {
        self.config.tries == 0 || self.attempts < self.config.tries
    }

    fn allows_method(&self, request: &Request) -> bool {
        !matches!(request.method.as_str(), "POST" | "LOCK" | "PATCH")
            || self.config.retry_non_idempotent()
    }

    fn within_timeout(&self) -> bool {
        self.config.timeout == Duration::ZERO || self.started.elapsed() < self.config.timeout
    }
}

#[derive(Debug)]
pub(super) struct ResolvedUpstreamTarget {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) peer: Option<UpstreamServer>,
    pub(super) check_health: bool,
}

#[derive(Debug, Clone)]
pub(super) struct UpstreamKeepaliveContext {
    pub(super) name: String,
    pub(super) config: UpstreamKeepaliveConfig,
}

impl ResolvedUpstreamTarget {
    pub(super) fn try_acquire(&self) -> Result<Option<UpstreamPeerGuard>, ()> {
        match &self.peer {
            Some(peer) => peer
                .try_acquire_with_health(self.check_health)
                .map(Some)
                .ok_or(()),
            None => Ok(None),
        }
    }
}

mod gateway;
mod grpc;
mod memcached;
mod proxy;
mod proxy_cookie_flags;
mod proxy_io;
mod proxy_request;
mod proxy_rewrite;
mod proxy_v2;
mod store;

pub(super) use gateway::*;
pub(super) use grpc::*;
pub(super) use memcached::*;
pub(super) use proxy::*;
pub(super) use proxy_cookie_flags::*;
pub(super) use proxy_io::*;
pub(super) use proxy_request::*;
pub(super) use proxy_rewrite::*;
pub(super) use proxy_v2::*;
pub(super) use store::*;
