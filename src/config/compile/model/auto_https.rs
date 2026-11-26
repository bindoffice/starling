use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoHttpsChallenge {
    #[default]
    Http01,
    Dns01,
}

impl AutoHttpsChallenge {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http01 => "http-01",
            Self::Dns01 => "dns-01",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoHttpsConfig {
    pub enabled: bool,
    pub cert_dir: PathBuf,
    pub email: Option<String>,
    pub staging: bool,
    pub acme_directory: Option<String>,
    pub acme_ca_file: Option<PathBuf>,
    pub challenge: AutoHttpsChallenge,
    pub dns_hook: Option<PathBuf>,
    pub dns_propagation_delay: Duration,
    pub redirect_http: bool,
    pub http_port: u16,
    pub https_port: u16,
    pub renew_before: Duration,
    pub check_interval: Duration,
}

impl AutoHttpsConfig {
    pub fn new(prefix: &Path) -> Self {
        Self {
            enabled: true,
            cert_dir: prefix.join("auto_https"),
            email: None,
            staging: false,
            acme_directory: None,
            acme_ca_file: None,
            challenge: AutoHttpsChallenge::Http01,
            dns_hook: None,
            dns_propagation_delay: Duration::from_secs(30),
            redirect_http: true,
            http_port: 80,
            https_port: 443,
            renew_before: Duration::from_secs(30 * 24 * 60 * 60),
            check_interval: Duration::from_secs(12 * 60 * 60),
        }
    }

    pub fn acme_directory_url(&self) -> &str {
        self.acme_directory.as_deref().unwrap_or(if self.staging {
            "https://acme-staging-v02.api.letsencrypt.org/directory"
        } else {
            "https://acme-v02.api.letsencrypt.org/directory"
        })
    }
}
