use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io;
use std::io::BufReader as StdBufReader;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use base64::Engine as _;
use bytes::Bytes;
use h2::RecvStream;
use h2::server::{self as h2_server, SendResponse};
use http::{Request as HttpRequest, Response as HttpResponse, StatusCode};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use tokio::io::{
    AsyncBufRead, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf,
};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{RwLock, mpsc, watch};
use tokio::task::JoinSet;
#[cfg(test)]
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::pki_types::{
    CertificateDer, CertificateRevocationListDer, PrivateKeyDer,
};
use tokio_rustls::rustls::server::{ClientHello, ResolvesServerCert};
use tokio_rustls::rustls::sign::CertifiedKey;
use tokio_rustls::{LazyConfigAcceptor, TlsConnector};

use crate::auth::{find_user_password, parse_authorization_header, verify_password};
use crate::config::{
    AccessLogDestination, AccessLogPath, AccessLogTarget, AccessRule, AutoindexFormat,
    ClientCertificateVerify, Condition, ConditionalConfig, DavMethod, DisableSymlinksMode,
    EffectiveConfig, ErrorPage, ErrorPageOverwrite, ExpiresOffset, ExpiresValue, FastCgiPassConfig,
    FileTest, GeoVariable, GrpcPassConfig, GzipDisableConfig, GzipProxiedConfig, GzipStatic,
    Http2Limits, HttpConfig, IfModifiedSince, LimitExcept, LimitReqLimit, LingeringClose,
    ListenAddress, ListenKeepalive, ListenSocketOptions, LocationConfig, LocationMatcher,
    LogEscapeMode, LogLevel, MapVariable, MemcachedPassConfig, NextUpstreamConfig,
    OpenLogFileCacheConfig, ProxyCookieFlagsRule, ProxyPassConfig, ProxyRedirectConfig,
    ProxyRedirectRule, RewriteAction, RewriteFlag, RuntimeConfig, STREAM_PROXY_RESPONSES_UNLIMITED,
    ScgiPassConfig, ServerConfig, ServerName, ServerTokens, SetVariable, SplitClientsVariable,
    StoreConfig, StreamHandler, TlsCipherConfig, TlsCipherSuite, TlsProtocols,
    UpstreamAttemptTiming, UpstreamConfig, UpstreamKeepaliveConfig, UpstreamPeerGuard,
    UpstreamServer, UserIdConfig, UserIdEnable, UserIdExpires, UwsgiPassConfig, match_location,
    normalize_uri, normalize_uri_with_merge, parse_size_value,
};
#[cfg(test)]
use crate::http::read_upstream_response_timed_with_rate;
use crate::http::{
    AccelBuffering, BodyBuffer, Header, MAX_BODY_BYTES, ReadRate, Request, RequestBodyFile,
    RequestHeadLimits, Response, ResponseBody, ResponseRate, ResponseStream, ResponseStreamItem,
    ResponseWriteOptions, TempFileGuard, UpstreamBodyBufferOptions, UpstreamBodyBufferPolicy,
    UpstreamBodyKind, UpstreamResponse, UpstreamResponseHead, error_response,
    parse_accel_buffering, prepare_response_for_write, prepare_response_headers,
    read_request_head_with_limits, read_upstream_response_body, read_upstream_response_head,
    reason_phrase, response_head_length, sanitize_header_value, store_request_body, write_response,
};
use crate::logging::{
    Logger, append_access_log, flush_access_logs, log_filter_allows, nginx_access_timestamp,
    render_log_format, render_log_format_bytes, reopen_access_logs, send_syslog,
};

mod early_hints;
mod tls;
mod user_agent;
use early_hints::*;
#[cfg(test)]
use tls::{
    CertificateResolver, build_client_cert_verifier, build_client_tls_config,
    client_certificate_info, load_certificate_paths, load_certificates, load_revocation_lists,
    load_root_certificates, rustls_protocol_versions, tls_crypto_provider,
};
use tls::{
    ClientTlsConfigCache, ClientTlsOptions, HttpTlsConfigs, StreamTlsConfigs, TlsConnectionInfo,
    accept_http_tls, accept_stream_tls, build_stream_tls_configs, build_tls_configs,
    tls_connection_info,
};
mod upstream;
use upstream::*;
mod variables;
use variables::*;
mod http2;
mod http_error_log;
mod listener_socket;
mod prefixed;
mod request;
mod resolver;
mod response;
mod socket_control;
mod temp_cleanup;
use http_error_log::*;
use http2::*;
use listener_socket::*;
use prefixed::*;
use request::*;
use response::*;
use socket_control::*;
use temp_cleanup::cleanup_stale_temp_files;
mod limits;
use limits::*;
mod stream;
#[cfg(test)]
use stream::{StreamRoute, handle_stream_connection, handle_stream_udp_session};
use stream::{
    build_stream_loggers, build_stream_routes, reopen_stream_loggers, stream_accept_loop,
    stream_udp_loop, stream_unix_accept_loop,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::server) enum ShutdownMode {
    Running,
    Graceful,
    Fast,
}

impl ShutdownMode {
    pub(in crate::server) fn requested(self) -> bool {
        self != Self::Running
    }
}

pub(in crate::server) async fn wait_for_shutdown(
    shutdown: &mut watch::Receiver<ShutdownMode>,
) -> ShutdownMode {
    let current = *shutdown.borrow_and_update();
    if current.requested() {
        return current;
    }
    match shutdown.changed().await {
        Ok(()) => *shutdown.borrow_and_update(),
        // A dropped sender is treated as an immediate shutdown request.
        Err(_) => ShutdownMode::Fast,
    }
}

pub(in crate::server) async fn wait_for_optional_shutdown(
    shutdown: &mut Option<watch::Receiver<ShutdownMode>>,
) -> ShutdownMode {
    match shutdown {
        Some(shutdown) => wait_for_shutdown(shutdown).await,
        None => std::future::pending().await,
    }
}

pub(in crate::server) fn shutdown_requested(
    shutdown: &Option<watch::Receiver<ShutdownMode>>,
) -> bool {
    requested_shutdown_mode(shutdown).is_some()
}

pub(in crate::server) fn requested_shutdown_mode(
    shutdown: &Option<watch::Receiver<ShutdownMode>>,
) -> Option<ShutdownMode> {
    shutdown
        .as_ref()
        .map(|shutdown| *shutdown.borrow())
        .filter(|mode| mode.requested())
}

#[derive(Clone)]
pub struct SharedState {
    config: Arc<ArcSwap<RuntimeSnapshot>>,
    reload: Arc<watch::Sender<u64>>,
    tls_configs: Arc<RwLock<HashMap<ListenAddress, Arc<HttpTlsConfigs>>>>,
    stream_tls_configs: Arc<RwLock<HashMap<ListenAddress, Arc<StreamTlsConfigs>>>>,
    logger: Arc<Logger>,
    http_error_loggers: Arc<RwLock<HttpErrorLoggers>>,
    stream_loggers: Arc<RwLock<HashMap<usize, Arc<Logger>>>>,
    limits: Arc<Mutex<LimitState>>,
    stats: Arc<ConnectionStats>,
    upstream_keepalive: Arc<UpstreamKeepalivePool>,
    proxy_cache: Arc<ProxyCacheRegistry>,
    client_tls_cache: Arc<ClientTlsConfigCache>,
}

#[derive(Clone)]
pub(in crate::server) struct RuntimeSnapshot {
    pub(in crate::server) config: Arc<RuntimeConfig>,
    pub(in crate::server) upstream_keepalive_generation: u64,
}

impl SharedState {
    /// Pair a request config with its keepalive generation in one atomic load.
    pub(in crate::server) fn config_snapshot(&self) -> RuntimeSnapshot {
        let snapshot = self.config.load();
        RuntimeSnapshot {
            config: snapshot.config.clone(),
            upstream_keepalive_generation: snapshot.upstream_keepalive_generation,
        }
    }

    pub(in crate::server) fn current_config(&self) -> Arc<RuntimeConfig> {
        self.config.load().config.clone()
    }

    pub(in crate::server) fn reload_receiver(&self) -> watch::Receiver<u64> {
        self.reload.subscribe()
    }

    pub(in crate::server) fn notify_reload(&self) {
        self.reload
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }
}

#[derive(Default)]
struct ConnectionStats {
    accepted: AtomicU64,
    handled: AtomicU64,
    requests: AtomicU64,
    active: AtomicU64,
    reading: AtomicU64,
    writing: AtomicU64,
    waiting: AtomicU64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnectionPhase {
    Idle,
    Reading,
    Waiting,
    Writing,
}

struct ConnectionStatsGuard {
    stats: Arc<ConnectionStats>,
    phase: Option<ConnectionPhase>,
    id: u64,
}

impl ConnectionStats {
    fn connection_started(self: &Arc<Self>) -> ConnectionStatsGuard {
        let id = self.accepted.fetch_add(1, Ordering::Relaxed) + 1;
        self.handled.fetch_add(1, Ordering::Relaxed);
        self.active.fetch_add(1, Ordering::Relaxed);
        ConnectionStatsGuard {
            stats: self.clone(),
            phase: Some(ConnectionPhase::Idle),
            id,
        }
    }
}

impl ConnectionStatsGuard {
    fn id(&self) -> u64 {
        self.id
    }

    fn set_idle(&mut self) {
        self.transition(ConnectionPhase::Idle);
    }

    fn set_reading(&mut self) {
        self.transition(ConnectionPhase::Reading);
    }

    fn set_writing(&mut self) {
        self.transition(ConnectionPhase::Writing);
    }

    fn set_waiting(&mut self) {
        self.transition(ConnectionPhase::Waiting);
    }

    fn transition(&mut self, next: ConnectionPhase) {
        if self.phase == Some(next) {
            return;
        }
        match self.phase {
            Some(ConnectionPhase::Reading) => {
                self.stats.reading.fetch_sub(1, Ordering::Relaxed);
            }
            Some(ConnectionPhase::Waiting) => {
                self.stats.waiting.fetch_sub(1, Ordering::Relaxed);
            }
            Some(ConnectionPhase::Writing) => {
                self.stats.writing.fetch_sub(1, Ordering::Relaxed);
            }
            Some(ConnectionPhase::Idle) | None => {}
        }
        match next {
            ConnectionPhase::Reading => {
                self.stats.reading.fetch_add(1, Ordering::Relaxed);
            }
            ConnectionPhase::Waiting => {
                self.stats.waiting.fetch_add(1, Ordering::Relaxed);
            }
            ConnectionPhase::Writing => {
                self.stats.writing.fetch_add(1, Ordering::Relaxed);
            }
            ConnectionPhase::Idle => {}
        }
        self.phase = Some(next);
    }
}

impl Drop for ConnectionStatsGuard {
    fn drop(&mut self) {
        match self.phase.take() {
            Some(ConnectionPhase::Reading) => {
                self.stats.reading.fetch_sub(1, Ordering::Relaxed);
            }
            Some(ConnectionPhase::Waiting) => {
                self.stats.waiting.fetch_sub(1, Ordering::Relaxed);
            }
            Some(ConnectionPhase::Writing) => {
                self.stats.writing.fetch_sub(1, Ordering::Relaxed);
            }
            Some(ConnectionPhase::Idle) => {}
            None => {}
        }
        self.stats.active.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone)]
struct ListenerGroup {
    address: ListenAddress,
    server_ids: Vec<usize>,
    default_server_id: usize,
    ssl: bool,
    http2: bool,
    proxy_protocol: bool,
    socket_options: ListenSocketOptions,
}

#[derive(Clone)]
struct ConnectionAddresses {
    remote: ListenAddress,
    local: ListenAddress,
    proxy_protocol_addr: Option<IpAddr>,
    proxy_protocol_port: Option<u16>,
    proxy_protocol_server_addr: Option<IpAddr>,
    proxy_protocol_server_port: Option<u16>,
    proxy_protocol_tlvs: Option<Arc<[u8]>>,
    tls: Option<TlsConnectionInfo>,
}

impl ConnectionAddresses {
    fn direct(remote: ListenAddress, local: ListenAddress) -> Self {
        Self {
            remote,
            local,
            proxy_protocol_addr: None,
            proxy_protocol_port: None,
            proxy_protocol_server_addr: None,
            proxy_protocol_server_port: None,
            proxy_protocol_tlvs: None,
            tls: None,
        }
    }
}

type ListenerSignature = (
    Vec<(ListenAddress, bool, bool, bool, ListenSocketOptions)>,
    Vec<(ListenAddress, bool, bool, bool, ListenSocketOptions)>,
);

const PROXY_PATH_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

mod accept;
mod helpers;
mod listeners;
mod run;
mod shutdown;

use accept::*;
use helpers::*;
use listeners::*;
pub use run::run;
use shutdown::*;

#[cfg(test)]
mod tests;
