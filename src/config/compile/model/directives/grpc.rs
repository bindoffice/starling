use super::*;

#[derive(Debug, Clone)]
pub struct GrpcPassConfig {
    pub raw: String,
    pub tls: bool,
    pub dynamic: bool,
    pub host: String,
    pub port: u16,
    pub authority: String,
    pub upstream: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GrpcConfig {
    pub pass: Option<GrpcPassConfig>,
    pub headers: Vec<HeaderValue>,
    pub hide_headers: Vec<String>,
    pub pass_headers: Vec<String>,
    pub ignore_headers: Vec<String>,
    pub connect_timeout: Duration,
    pub send_timeout: Duration,
    pub read_timeout: Duration,
    pub buffer_size: usize,
    pub request_buffering: bool,
    pub ignore_client_abort: bool,
    pub next_upstream: NextUpstreamConfig,
    pub intercept_errors: bool,
    pub bind: Option<String>,
    pub socket_keepalive: bool,
    pub socket_rcvbuf: usize,
    pub socket_sndbuf: usize,
    pub ssl_server_name: bool,
    pub ssl_name: Option<String>,
    pub ssl_session_reuse: bool,
    pub ssl_verify: bool,
    pub ssl_trusted_certificate: Option<PathBuf>,
    pub ssl_verify_depth: u32,
    pub ssl_crl: Option<PathBuf>,
    pub ssl_certificate: Option<PathBuf>,
    pub ssl_certificate_key: Option<PathBuf>,
    pub ssl_passwords: Vec<Vec<u8>>,
    pub ssl_protocols: TlsProtocols,
    pub ssl_ciphers: TlsCipherConfig,
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            pass: None,
            headers: Vec::new(),
            hide_headers: Vec::new(),
            pass_headers: Vec::new(),
            ignore_headers: Vec::new(),
            connect_timeout: Duration::from_secs(60),
            send_timeout: Duration::from_secs(60),
            read_timeout: Duration::from_secs(60),
            buffer_size: 4096,
            request_buffering: true,
            ignore_client_abort: false,
            next_upstream: NextUpstreamConfig::default(),
            intercept_errors: false,
            bind: None,
            socket_keepalive: false,
            socket_rcvbuf: 0,
            socket_sndbuf: 0,
            ssl_server_name: false,
            ssl_name: None,
            ssl_session_reuse: true,
            ssl_verify: false,
            ssl_trusted_certificate: None,
            ssl_verify_depth: 1,
            ssl_crl: None,
            ssl_certificate: None,
            ssl_certificate_key: None,
            ssl_passwords: Vec::new(),
            ssl_protocols: TlsProtocols::default(),
            ssl_ciphers: TlsCipherConfig::default(),
        }
    }
}

#[derive(Default)]
pub(in crate::config::compile) struct GrpcSeen {
    pub(in crate::config::compile) header: bool,
    pub(in crate::config::compile) hide_header: bool,
    pub(in crate::config::compile) pass_header: bool,
    pub(in crate::config::compile) ignore_headers: bool,
    pub(in crate::config::compile) ssl_protocols: bool,
    pub(in crate::config::compile) request_buffering: bool,
    pub(in crate::config::compile) ignore_client_abort: bool,
}
