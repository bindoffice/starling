use std::net::SocketAddr;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverConfig {
    pub servers: Vec<SocketAddr>,
    pub valid: Option<Duration>,
    pub ipv6: bool,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            valid: None,
            ipv6: true,
        }
    }
}

impl ResolverConfig {
    pub fn is_configured(&self) -> bool {
        !self.servers.is_empty()
    }
}
