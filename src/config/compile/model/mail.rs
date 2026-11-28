use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use super::*;

#[derive(Debug, Clone)]
pub struct MailConfig {
    pub servers: Vec<MailServer>,
    pub error_logs: Vec<LogTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailProtocol {
    Pop3,
    Imap,
    Smtp,
}

impl MailProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pop3 => "pop3",
            Self::Imap => "imap",
            Self::Smtp => "smtp",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Self::Pop3 => 110,
            Self::Imap => 143,
            Self::Smtp => 25,
        }
    }

    pub fn alpn(self) -> &'static [u8] {
        match self {
            Self::Pop3 => b"pop3",
            Self::Imap => b"imap",
            Self::Smtp => b"smtp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MailAuthMethod {
    Plain,
    Login,
    Apop,
    CramMd5,
    External,
    None,
}

impl MailAuthMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Login => "login",
            Self::Apop => "apop",
            Self::CramMd5 => "cram-md5",
            Self::External => "external",
            Self::None => "none",
        }
    }

    pub fn auth_http_name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Login => "login",
            Self::Apop => "apop",
            Self::CramMd5 => "cram-md5",
            Self::External => "external",
            Self::None => "none",
        }
    }

    pub fn canonical_order(self) -> usize {
        match self {
            Self::Plain => 0,
            Self::Login => 1,
            Self::Apop => 2,
            Self::CramMd5 => 3,
            Self::External => 4,
            Self::None => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MailStartTls {
    #[default]
    Off,
    On,
    Only,
}

#[derive(Debug, Clone)]
pub enum MailAuthHttpAddress {
    Inet(Vec<SocketAddr>),
    Unix(PathBuf),
}

#[derive(Debug, Clone)]
pub struct MailAuthHttpConfig {
    pub address: MailAuthHttpAddress,
    pub host_header: String,
    pub uri: String,
    pub timeout: Duration,
    pub headers: Vec<(String, String)>,
    pub pass_client_cert: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MailProxyProtocol {
    #[default]
    Off,
    On,
    V2,
}

#[derive(Debug, Clone)]
pub struct MailProxyConfig {
    pub enabled: bool,
    pub timeout: Duration,
    pub buffer_size: usize,
    pub pass_error_message: bool,
    pub proxy_protocol: MailProxyProtocol,
    pub smtp_auth: bool,
    pub xclient: bool,
}

impl Default for MailProxyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: Duration::from_secs(24 * 60 * 60),
            buffer_size: 4096,
            pass_error_message: false,
            proxy_protocol: MailProxyProtocol::Off,
            smtp_auth: false,
            xclient: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MailTlsConfig {
    pub certificate: Option<PathBuf>,
    pub certificate_key: Option<PathBuf>,
    pub certificates: Vec<PathBuf>,
    pub certificate_keys: Vec<PathBuf>,
    pub protocols: TlsProtocols,
    pub prefer_server_ciphers: bool,
    pub ciphers: TlsCipherConfig,
    pub passwords: Vec<Vec<u8>>,
    pub verify_client: ClientCertificateVerify,
    pub verify_depth: u32,
    pub client_certificate: Option<PathBuf>,
    pub trusted_certificate: Option<PathBuf>,
    pub crl: Option<PathBuf>,
    pub starttls: MailStartTls,
    pub session_cache: MailSessionCache,
    pub session_timeout: Duration,
    pub session_tickets: bool,
}

impl Default for MailTlsConfig {
    fn default() -> Self {
        Self {
            certificate: None,
            certificate_key: None,
            certificates: Vec::new(),
            certificate_keys: Vec::new(),
            protocols: TlsProtocols::default(),
            prefer_server_ciphers: false,
            ciphers: TlsCipherConfig::default(),
            passwords: Vec::new(),
            verify_client: ClientCertificateVerify::Off,
            verify_depth: 1,
            client_certificate: None,
            trusted_certificate: None,
            crl: None,
            starttls: MailStartTls::Off,
            session_cache: MailSessionCache::None,
            session_timeout: Duration::from_secs(5 * 60),
            session_tickets: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailSessionCache {
    Off,
    None,
    Builtin(Option<usize>),
    Shared { name: String, size: usize },
}

#[derive(Debug, Clone)]
pub struct MailServer {
    pub id: usize,
    pub listens: Vec<ListenConfig>,
    pub protocol: MailProtocol,
    pub real_ip_from: Vec<RealIpSource>,
    pub server_name: String,
    pub timeout: Duration,
    pub max_errors: usize,
    pub error_logs: Vec<LogTarget>,
    pub resolver: ResolverConfig,
    pub resolver_timeout: Duration,
    pub auth_http: MailAuthHttpConfig,
    pub auth_methods: Vec<MailAuthMethod>,
    pub capabilities: Vec<String>,
    pub client_buffer_size: usize,
    pub greeting_delay: Duration,
    pub proxy: MailProxyConfig,
    pub tls: MailTlsConfig,
}

impl MailServer {
    pub fn has_auth_method(&self, method: MailAuthMethod) -> bool {
        self.auth_methods.contains(&method)
    }
}
