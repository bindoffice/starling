use super::*;
use crate::error::SourceLocation;
use std::fmt;

mod dav;
pub use dav::*;
mod cache;
pub use cache::*;
mod listen;
pub use listen::*;
mod memcached;
pub use memcached::*;

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub prefix: PathBuf,
    pub config_path: PathBuf,
    pub files: Vec<PathBuf>,
    pub warnings: Vec<ConfigWarning>,
    pub core: CoreConfig,
    pub http: Option<HttpConfig>,
    pub stream: Option<StreamConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigWarning {
    pub location: SourceLocation,
    pub message: String,
}

impl ConfigWarning {
    pub fn at(location: SourceLocation, message: impl Into<String>) -> Self {
        Self {
            location,
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.location, self.message)
    }
}

#[derive(Debug, Clone)]
pub struct CoreConfig {
    pub daemon: bool,
    pub master_process: bool,
    pub worker_processes: WorkerProcesses,
    pub worker_shutdown_timeout: Option<std::time::Duration>,
    pub pid: PathBuf,
    pub error_logs: Vec<LogTarget>,
    pub working_directory: Option<PathBuf>,
    pub worker_connections: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum WorkerProcesses {
    Auto,
    Count(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LogTarget {
    pub destination: LogDestination,
    pub level: LogLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LogDestination {
    Stderr,
    File(PathBuf),
    Syslog(SyslogConfig),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    Debug,
    Info,
    Notice,
    Warn,
    Error,
    Crit,
    Alert,
    Emerg,
}

impl LogLevel {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "notice" => Some(Self::Notice),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            "crit" => Some(Self::Crit),
            "alert" => Some(Self::Alert),
            "emerg" => Some(Self::Emerg),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Notice => "notice",
            Self::Warn => "warn",
            Self::Error => "error",
            Self::Crit => "crit",
            Self::Alert => "alert",
            Self::Emerg => "emerg",
        }
    }

    pub fn syslog_severity(self) -> u8 {
        match self {
            Self::Emerg => 0,
            Self::Alert => 1,
            Self::Crit => 2,
            Self::Error => 3,
            Self::Warn => 4,
            Self::Notice => 5,
            Self::Info => 6,
            Self::Debug => 7,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub servers: Vec<ServerConfig>,
    pub error_logs: Vec<LogTarget>,
    pub upstreams: HashMap<String, UpstreamConfig>,
    pub cache_paths: HashMap<String, Arc<ProxyCachePathConfig>>,
    pub maps: HashMap<String, Arc<MapVariable>>,
    pub geos: HashMap<String, Arc<GeoVariable>>,
    pub split_clients: HashMap<String, Arc<SplitClientsVariable>>,
    pub limit_req_zones: HashMap<String, Arc<LimitReqZone>>,
    pub limit_conn_zones: HashMap<String, Arc<LimitConnZone>>,
    pub mime_types: HashMap<String, String>,
    pub default_type: String,
    pub log_formats: HashMap<String, AccessLogFormat>,
}

#[derive(Debug, Clone)]
pub struct StreamConfig {
    pub servers: Vec<StreamServer>,
    pub error_logs: Vec<LogTarget>,
    pub upstreams: HashMap<String, UpstreamConfig>,
    pub maps: Arc<HashMap<String, Arc<MapVariable>>>,
    pub geos: Arc<HashMap<String, Arc<GeoVariable>>>,
    pub split_clients: Arc<HashMap<String, Arc<SplitClientsVariable>>>,
    pub limit_conn_zones: Arc<HashMap<String, Arc<LimitConnZone>>>,
    pub log_formats: HashMap<String, AccessLogFormat>,
}

#[derive(Debug, Clone)]
pub struct StreamServer {
    pub id: usize,
    pub listens: Vec<ListenConfig>,
    pub server_name: String,
    pub names: Vec<ServerName>,
    pub error_logs: Vec<LogTarget>,
    pub access_log: AccessLogConfig,
    pub open_log_file_cache: Option<Arc<OpenLogFileCacheConfig>>,
    pub access_rules: Vec<AccessRule>,
    pub real_ip_from: Vec<RealIpSource>,
    pub tcp_nodelay: bool,
    pub ssl_preread: bool,
    pub preread_timeout: Duration,
    pub preread_buffer_size: usize,
    pub proxy_pass: Option<StreamProxyPass>,
    pub return_value: Option<String>,
    pub pass: Option<StreamPass>,
    pub handler: StreamHandler,
    pub sets: Vec<SetVariable>,
    pub limit_conns: Vec<LimitConnLimit>,
    pub limit_conn_dry_run: bool,
    pub limit_conn_log_level: LogLevel,
    pub proxy_protocol: StreamProxyProtocol,
    pub proxy_protocol_timeout: Duration,
    pub proxy_half_close: bool,
    pub proxy_timeout: Duration,
    pub proxy_connect_timeout: Duration,
    pub proxy_requests: usize,
    pub proxy_responses: usize,
    pub proxy_next_upstream: bool,
    pub proxy_next_upstream_tries: usize,
    pub proxy_next_upstream_timeout: Duration,
    pub proxy_upload_rate: Option<String>,
    pub proxy_download_rate: Option<String>,
    pub proxy_buffer_size: usize,
    pub proxy_socket_keepalive: bool,
    pub proxy_socket_rcvbuf: usize,
    pub proxy_socket_sndbuf: usize,
    pub proxy_bind: Option<String>,
    pub proxy_ssl: bool,
    pub proxy_ssl_server_name: bool,
    pub proxy_ssl_name: Option<String>,
    pub proxy_ssl_session_reuse: bool,
    pub proxy_ssl_verify: bool,
    pub proxy_ssl_trusted_certificate: Option<PathBuf>,
    pub proxy_ssl_verify_depth: u32,
    pub proxy_ssl_crl: Option<PathBuf>,
    pub proxy_ssl_certificate: Option<PathBuf>,
    pub proxy_ssl_certificate_key: Option<PathBuf>,
    pub proxy_ssl_passwords: Vec<Vec<u8>>,
    pub proxy_ssl_alpn: Vec<String>,
    pub proxy_ssl_protocols: TlsProtocols,
    pub proxy_ssl_ciphers: TlsCipherConfig,
    pub ssl_certificate: Option<PathBuf>,
    pub ssl_certificate_key: Option<PathBuf>,
    pub ssl_alpn: Vec<String>,
    pub ssl_passwords: Vec<Vec<u8>>,
    pub ssl_handshake_timeout: Duration,
    pub ssl_reject_handshake: bool,
    pub ssl_verify_client: ClientCertificateVerify,
    pub ssl_verify_depth: u32,
    pub ssl_client_certificate: Option<PathBuf>,
    pub ssl_trusted_certificate: Option<PathBuf>,
    pub ssl_crl: Option<PathBuf>,
    pub ssl_stapling: bool,
    pub ssl_stapling_verify: bool,
    pub ssl_stapling_file: Option<PathBuf>,
    pub ssl_stapling_responder: Option<String>,
    pub resolver: ResolverConfig,
    pub resolver_timeout: Duration,
    pub ssl_protocols: TlsProtocols,
    pub ssl_prefer_server_ciphers: bool,
    pub ssl_ciphers: TlsCipherConfig,
}

pub const STREAM_PROXY_RESPONSES_UNLIMITED: usize = i32::MAX as usize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StreamProxyProtocol {
    #[default]
    Off,
    V1,
    V2,
}

#[derive(Debug, Clone)]
pub struct StreamProxyPass {
    pub target: String,
    pub upstream: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub id: usize,
    pub listens: Vec<ListenConfig>,
    pub http2: bool,
    pub names: Vec<ServerName>,
    pub context: EffectiveConfig,
    pub locations: Vec<LocationConfig>,
}

#[derive(Debug, Clone)]
pub enum ServerName {
    Exact(String),
    LeadingWildcard { value: String, display: String },
    TrailingWildcard { value: String, display: String },
    Regex { regex: Regex, display: String },
}

impl ServerName {
    pub fn matches(&self, host: &str) -> bool {
        match self {
            Self::Exact(name) => name.eq_ignore_ascii_case(host),
            Self::LeadingWildcard { value: suffix, .. } => {
                let bare = suffix.strip_prefix('.').unwrap_or(suffix);
                host.eq_ignore_ascii_case(bare)
                    || (host.len() > suffix.len()
                        && host
                            .get(host.len() - suffix.len()..)
                            .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix)))
            }
            Self::TrailingWildcard { value: prefix, .. } => {
                let bare = prefix.strip_suffix('.').unwrap_or(prefix);
                host.eq_ignore_ascii_case(bare)
                    || (host.len() > prefix.len()
                        && host
                            .get(..prefix.len())
                            .is_some_and(|head| head.eq_ignore_ascii_case(prefix)))
            }
            Self::Regex { regex, .. } => regex.is_match(host),
        }
    }

    pub fn display(&self) -> &str {
        match self {
            Self::Exact(value) => value,
            Self::LeadingWildcard { display, .. }
            | Self::TrailingWildcard { display, .. }
            | Self::Regex { display, .. } => display,
        }
    }

    pub fn regex(&self) -> Option<&Regex> {
        match self {
            Self::Regex { regex, .. } => Some(regex),
            _ => None,
        }
    }

    pub fn wildcard_specificity(&self, host: &str) -> Option<usize> {
        match self {
            Self::LeadingWildcard { value, .. } | Self::TrailingWildcard { value, .. }
                if self.matches(host) =>
            {
                Some(value.len())
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocationConfig {
    pub matcher: LocationMatcher,
    pub context: EffectiveConfig,
    pub nested: Vec<LocationConfig>,
    pub location: SourceLocation,
}

#[derive(Debug, Clone)]
pub enum LocationMatcher {
    Exact(String),
    Prefix(String),
    PrefixNoRegex(String),
    Regex(Regex),
    Named(String),
}

pub(super) const NEXT_UPSTREAM_ERROR: u32 = 1 << 0;
pub(super) const NEXT_UPSTREAM_TIMEOUT: u32 = 1 << 1;
pub(super) const NEXT_UPSTREAM_INVALID_HEADER: u32 = 1 << 2;
pub(super) const NEXT_UPSTREAM_NON_IDEMPOTENT: u32 = 1 << 3;
pub(super) const NEXT_UPSTREAM_HTTP_500: u32 = 1 << 4;
pub(super) const NEXT_UPSTREAM_HTTP_502: u32 = 1 << 5;
pub(super) const NEXT_UPSTREAM_HTTP_503: u32 = 1 << 6;
pub(super) const NEXT_UPSTREAM_HTTP_504: u32 = 1 << 7;
pub(super) const NEXT_UPSTREAM_HTTP_403: u32 = 1 << 8;
pub(super) const NEXT_UPSTREAM_HTTP_404: u32 = 1 << 9;
pub(super) const NEXT_UPSTREAM_HTTP_429: u32 = 1 << 10;
pub(super) const NEXT_UPSTREAM_OFF: u32 = 1 << 31;

mod access_log;
mod autoindex;
mod browser;
mod charset;
mod directives;
mod disable_symlinks;
mod early_hints;
mod effective;
mod gzip;
mod http2;
mod keepalive;
mod proxy_cookie_flags;
mod referer;
mod request_body;
mod request_headers;
mod secure_link;
mod ssi;
mod store;
mod stream_pass;
mod syslog;
mod temp_path;
mod tls;
mod tls_stapling;
mod upstream;
mod upstream_buffering;
mod userid;

pub use access_log::*;
pub use autoindex::*;
pub use browser::*;
pub use charset::*;
pub use directives::*;
pub use disable_symlinks::*;
pub use early_hints::*;
pub use effective::*;
pub use gzip::*;
pub use http2::*;
pub use keepalive::*;
pub use proxy_cookie_flags::*;
pub use referer::*;
pub use request_body::*;
pub use request_headers::*;
pub use secure_link::*;
pub use ssi::*;
pub use store::*;
pub use stream_pass::*;
pub use syslog::*;
pub use temp_path::*;
pub use tls::*;
pub use tls_stapling::*;
pub use upstream::*;
pub use upstream_buffering::*;
pub use userid::*;
