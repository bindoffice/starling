use super::*;

pub const TUNNEL_DEFAULT_BUFFER_SIZE: usize = 4096;
pub const TUNNEL_DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelPassConfig {
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelConfig {
    pub pass: Option<TunnelPassConfig>,
    pub bind: Option<String>,
    pub socket_keepalive: bool,
    pub socket_rcvbuf: usize,
    pub socket_sndbuf: usize,
    pub connect_timeout: Duration,
    pub send_timeout: Duration,
    pub read_timeout: Duration,
    pub send_lowat: usize,
    pub buffer_size: usize,
    pub next_upstream: NextUpstreamConfig,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            pass: None,
            bind: None,
            socket_keepalive: false,
            socket_rcvbuf: 0,
            socket_sndbuf: 0,
            connect_timeout: TUNNEL_DEFAULT_TIMEOUT,
            send_timeout: TUNNEL_DEFAULT_TIMEOUT,
            read_timeout: TUNNEL_DEFAULT_TIMEOUT,
            send_lowat: 0,
            buffer_size: TUNNEL_DEFAULT_BUFFER_SIZE,
            next_upstream: NextUpstreamConfig::default(),
        }
    }
}
