use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ListenConfig {
    pub address: ListenAddress,
    pub ssl: bool,
    pub http2: bool,
    pub quic: bool,
    pub udp: bool,
    pub proxy_protocol: bool,
    pub default_server: bool,
    pub socket_options: ListenSocketOptions,
    pub socket_options_set: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListenSocketOptions {
    pub backlog: u32,
    pub backlog_set: bool,
    pub receive_buffer_size: Option<usize>,
    pub send_buffer_size: Option<usize>,
    pub reuseport: bool,
    pub bind: bool,
    pub multipath: bool,
    pub ipv6only: bool,
    pub keepalive: ListenKeepalive,
    pub fastopen: Option<u32>,
    pub deferred: bool,
}

impl Default for ListenSocketOptions {
    fn default() -> Self {
        Self {
            backlog: 511,
            backlog_set: false,
            receive_buffer_size: None,
            send_buffer_size: None,
            reuseport: false,
            bind: false,
            multipath: false,
            ipv6only: true,
            keepalive: ListenKeepalive::Unset,
            fastopen: None,
            deferred: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ListenKeepalive {
    Unset,
    On,
    Off,
    Tuned {
        idle: Option<Duration>,
        interval: Option<Duration>,
        count: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ListenAddress {
    Inet(std::net::SocketAddr),
    Unix(PathBuf),
}

impl ListenAddress {
    pub fn port(&self) -> u16 {
        match self {
            Self::Inet(address) => address.port(),
            Self::Unix(_) => 0,
        }
    }

    pub fn socket_addr(&self) -> Option<std::net::SocketAddr> {
        match self {
            Self::Inet(address) => Some(*address),
            Self::Unix(_) => None,
        }
    }

    pub fn unix_path(&self) -> Option<&Path> {
        match self {
            Self::Inet(_) => None,
            Self::Unix(path) => Some(path),
        }
    }
}

impl std::fmt::Display for ListenAddress {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inet(address) => write!(formatter, "{address}"),
            Self::Unix(path) => write!(formatter, "unix:{}", path.display()),
        }
    }
}
