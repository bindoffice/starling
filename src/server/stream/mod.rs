use super::*;

#[derive(Debug, Clone)]
pub(super) struct StreamRoute {
    pub(super) address: ListenAddress,
    pub(super) ssl: bool,
    pub(super) udp: bool,
    pub(super) proxy_protocol: bool,
    pub(super) socket_options: ListenSocketOptions,
}

pub(in crate::server) struct StreamLogContext {
    pub(in crate::server) logger: Arc<Logger>,
    pub(in crate::server) started: Instant,
    pub(in crate::server) remote: ListenAddress,
    pub(in crate::server) realip_remote: ListenAddress,
    pub(in crate::server) local: ListenAddress,
    pub(in crate::server) proxy_protocol_addr: Option<IpAddr>,
    pub(in crate::server) proxy_protocol_port: Option<u16>,
    pub(in crate::server) proxy_protocol_server_addr: Option<IpAddr>,
    pub(in crate::server) proxy_protocol_server_port: Option<u16>,
    pub(in crate::server) proxy_protocol_tlvs: Option<Arc<[u8]>>,
    pub(in crate::server) protocol: &'static str,
    pub(in crate::server) server_name: String,
    pub(in crate::server) limit_conn_status: String,
    pub(in crate::server) status: u16,
    pub(in crate::server) bytes_sent: u64,
    pub(in crate::server) bytes_received: u64,
    pub(in crate::server) upstream_addr: String,
    pub(in crate::server) upstream_bytes_sent: u64,
    pub(in crate::server) upstream_bytes_received: u64,
    pub(in crate::server) upstream_connect_time: Option<Duration>,
    pub(in crate::server) upstream_first_byte_time: Option<Duration>,
    pub(in crate::server) upstream_session_time: Option<Duration>,
    pub(in crate::server) tls: Option<TlsConnectionInfo>,
    pub(in crate::server) ssl_preread: Option<StreamSslPreread>,
    pub(in crate::server) variables: HashMap<String, String>,
    pub(in crate::server) maps: Arc<HashMap<String, Arc<MapVariable>>>,
    pub(in crate::server) geos: Arc<HashMap<String, Arc<GeoVariable>>>,
    pub(in crate::server) split_clients: Arc<HashMap<String, Arc<SplitClientsVariable>>>,
}

mod access;
mod attempt;
mod connect;
mod error_log;
mod geo_variable;
mod limits;
mod logging;
mod map_variable;
mod pass;
mod proxy_protocol;
mod proxy_protocol_ssl;
mod proxy_settings;
mod realip;
mod relay;
mod relay_copy;
mod retry;
mod return_directive;
mod routes;
mod socket_options;
mod split_clients_variable;
mod ssl_preread;
mod targets;
mod tcp;
mod udp;
mod udp_counts;
mod variables;
mod virtual_server;

pub(super) use access::*;
pub(super) use attempt::*;
pub(super) use connect::*;
pub(super) use error_log::*;
pub(super) use geo_variable::*;
pub(super) use limits::*;
pub(super) use logging::*;
pub(super) use map_variable::*;
pub(super) use pass::*;
pub(super) use proxy_protocol::*;
pub(super) use proxy_protocol_ssl::*;
pub(super) use proxy_settings::*;
pub(super) use realip::*;
pub(super) use relay::*;
pub(super) use relay_copy::*;
pub(super) use retry::*;
pub(super) use return_directive::*;
pub(super) use routes::*;
pub(super) use socket_options::*;
pub(super) use split_clients_variable::*;
pub(super) use ssl_preread::*;
pub(super) use targets::*;
pub(super) use tcp::*;
pub(super) use udp::*;
pub(super) use udp_counts::*;
pub(super) use variables::*;
pub(super) use virtual_server::*;
