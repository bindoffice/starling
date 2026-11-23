use super::*;

#[derive(Clone, Default)]
pub(super) struct UserIdState {
    pub(super) enable: UserIdEnable,
    pub(super) service: Option<u32>,
    pub(super) name: String,
    pub(super) got: [u32; 4],
    pub(super) set: [u32; 4],
    pub(super) cookie: Option<String>,
    pub(super) mark: Option<u8>,
    pub(super) reset: bool,
}

#[derive(Clone)]
pub(super) struct Variables {
    pub(super) method: String,
    pub(super) request_target: String,
    pub(super) version: String,
    pub(super) host: String,
    pub(super) server_name: String,
    pub(super) remote_addr: String,
    pub(super) remote_user: String,
    pub(super) remote_port: String,
    pub(super) server_addr: String,
    pub(super) server_port: String,
    pub(super) request_port: String,
    pub(super) is_request_port: String,
    pub(super) scheme: String,
    pub(super) https: String,
    pub(super) ssl_protocol: String,
    pub(super) ssl_cipher: String,
    pub(super) ssl_server_name: String,
    pub(super) ssl_alpn_protocol: String,
    pub(super) ssl_client_verify: String,
    pub(super) ssl_client_cert: String,
    pub(super) ssl_client_escaped_cert: String,
    pub(super) ssl_client_fingerprint: String,
    pub(super) ssl_client_serial: String,
    pub(super) ssl_client_s_dn: String,
    pub(super) ssl_client_i_dn: String,
    pub(super) ssl_client_s_dn_legacy: String,
    pub(super) ssl_client_i_dn_legacy: String,
    pub(super) ssl_client_v_start: String,
    pub(super) ssl_client_v_end: String,
    pub(super) ssl_client_v_remain: String,
    pub(super) hostname: String,
    pub(super) request_id: String,
    pub(super) realip_remote_addr: String,
    pub(super) realip_remote_port: String,
    pub(super) proxy_protocol_addr: String,
    pub(super) proxy_protocol_port: String,
    pub(super) proxy_protocol_server_addr: String,
    pub(super) proxy_protocol_server_port: String,
    pub(super) proxy_protocol_tlvs: Option<Arc<[u8]>>,
    pub(super) uri: String,
    pub(super) fastcgi_script_name: String,
    pub(super) fastcgi_path_info: String,
    pub(super) uwsgi_script_name: String,
    pub(super) scgi_script_name: String,
    pub(super) args: String,
    pub(super) status: u16,
    pub(super) body_bytes_sent: u64,
    pub(super) content_length: String,
    pub(super) content_type: String,
    pub(super) request_body: String,
    pub(super) request_body_file: String,
    pub(super) request_body_file_guard: Option<Arc<RequestBodyFile>>,
    pub(super) request_body_prepared: bool,
    pub(super) request_length: String,
    pub(super) upstream_status: String,
    pub(super) upstream_cache_status: String,
    pub(super) proxy_cache_background: bool,
    pub(super) upstream_response_time: String,
    pub(super) upstream_headers: Vec<Header>,
    pub(super) upstream_trailers: Vec<Header>,
    pub(super) sent_headers: Vec<Header>,
    pub(super) sent_trailers: Vec<Header>,
    pub(super) sent_connection: String,
    pub(super) sent_keep_alive: String,
    pub(super) sent_transfer_encoding: String,
    pub(super) response_from_upstream: bool,
    pub(super) client_abort: Option<ClientAbortState>,
    pub(super) error_page: bool,
    pub(super) started: Instant,
    pub(super) limit_req_status: String,
    pub(super) limit_conn_status: String,
    pub(super) invalid_referer: String,
    pub(super) secure_link: String,
    pub(super) secure_link_expires: String,
    pub(super) msie: String,
    pub(super) modern_browser: String,
    pub(super) ancient_browser: String,
    pub(super) userid: UserIdState,
    pub(super) document_root: PathBuf,
    pub(super) request_filename: PathBuf,
    pub(super) proxy_host: String,
    pub(super) proxy_port: String,
    pub(super) upstream_addr: String,
    pub(super) headers: Vec<Header>,
    pub(super) captures: Vec<String>,
    pub(super) server_names: Vec<ServerName>,
    pub(super) custom: HashMap<String, String>,
    pub(super) maps: Arc<HashMap<String, Arc<MapVariable>>>,
    pub(super) geos: Arc<HashMap<String, Arc<GeoVariable>>>,
    pub(super) split_clients: Arc<HashMap<String, Arc<SplitClientsVariable>>>,
    pub(super) limits: Arc<Mutex<LimitState>>,
    pub(super) stats: Arc<ConnectionStats>,
    pub(super) upstream_keepalive: Arc<UpstreamKeepalivePool>,
    pub(super) upstream_keepalive_generation: u64,
    pub(super) proxy_cache: Arc<ProxyCacheRegistry>,
    pub(super) client_tls_cache: Arc<ClientTlsConfigCache>,
    pub(super) error_logs: Option<RequestErrorLogs>,
    pub(super) rewrite_logger: Option<Arc<Logger>>,
    pub(super) rewrite_log: bool,
    pub(super) uninitialized_variable_warn: bool,
}

#[derive(Clone)]
pub(super) struct RequestErrorLogs {
    default: Arc<Logger>,
    by_targets: Arc<HashMap<Vec<crate::config::LogTarget>, Arc<Logger>>>,
}

impl RequestErrorLogs {
    pub(super) fn new(
        default: Arc<Logger>,
        by_targets: Arc<HashMap<Vec<crate::config::LogTarget>, Arc<Logger>>>,
    ) -> Self {
        Self {
            default,
            by_targets,
        }
    }

    fn for_context(&self, context: &EffectiveConfig) -> Arc<Logger> {
        if context.error_logs.is_empty() {
            return self.default.clone();
        }
        self.by_targets
            .get(&context.error_logs)
            .cloned()
            .unwrap_or_else(|| self.default.clone())
    }
}

pub(super) trait AddressVariables {
    fn variable_address(&self) -> String;
    fn variable_port(&self) -> String;
}

impl AddressVariables for SocketAddr {
    fn variable_address(&self) -> String {
        self.ip().to_string()
    }

    fn variable_port(&self) -> String {
        self.port().to_string()
    }
}

impl AddressVariables for ListenAddress {
    fn variable_address(&self) -> String {
        match self {
            Self::Inet(address) => address.ip().to_string(),
            Self::Unix(path) => format!("unix:{}", path.display()),
        }
    }

    fn variable_port(&self) -> String {
        match self {
            Self::Inet(address) => address.port().to_string(),
            Self::Unix(_) => String::new(),
        }
    }
}

impl Variables {
    pub(super) fn new<L: AddressVariables + ?Sized, R: AddressVariables + ?Sized>(
        request: &Request,
        host: &str,
        server: &ServerConfig,
        local: &L,
        remote: &R,
    ) -> Self {
        let request_port = request.request_port();
        Self {
            method: request.method.clone(),
            request_target: request.target.clone(),
            version: request.version.clone(),
            host: host.to_string(),
            server_name: server
                .names
                .first()
                .map(server_name_value)
                .unwrap_or_default(),
            remote_addr: remote.variable_address(),
            remote_user: request
                .header("authorization")
                .and_then(parse_authorization_header)
                .map(|credentials| credentials.username)
                .unwrap_or_default(),
            remote_port: remote.variable_port(),
            server_addr: local.variable_address(),
            server_port: local.variable_port(),
            request_port: request_port
                .map(|port| port.to_string())
                .unwrap_or_default(),
            is_request_port: request_port.map(|_| ":").unwrap_or_default().into(),
            scheme: "http".into(),
            https: String::new(),
            ssl_protocol: String::new(),
            ssl_cipher: String::new(),
            ssl_server_name: String::new(),
            ssl_alpn_protocol: String::new(),
            ssl_client_verify: String::new(),
            ssl_client_cert: String::new(),
            ssl_client_escaped_cert: String::new(),
            ssl_client_fingerprint: String::new(),
            ssl_client_serial: String::new(),
            ssl_client_s_dn: String::new(),
            ssl_client_i_dn: String::new(),
            ssl_client_s_dn_legacy: String::new(),
            ssl_client_i_dn_legacy: String::new(),
            ssl_client_v_start: String::new(),
            ssl_client_v_end: String::new(),
            ssl_client_v_remain: String::new(),
            hostname: hostname(),
            request_id: next_request_id(),
            realip_remote_addr: remote.variable_address(),
            realip_remote_port: remote.variable_port(),
            proxy_protocol_addr: String::new(),
            proxy_protocol_port: String::new(),
            proxy_protocol_server_addr: String::new(),
            proxy_protocol_server_port: String::new(),
            proxy_protocol_tlvs: None,
            uri: request.path_and_query().0.to_string(),
            fastcgi_script_name: request.path_and_query().0.to_string(),
            fastcgi_path_info: String::new(),
            uwsgi_script_name: request.path_and_query().0.to_string(),
            scgi_script_name: request.path_and_query().0.to_string(),
            args: request.path_and_query().1.unwrap_or_default().to_string(),
            status: 0,
            body_bytes_sent: 0,
            content_length: request
                .header("content-length")
                .unwrap_or_default()
                .to_string(),
            content_type: request
                .header("content-type")
                .unwrap_or_default()
                .to_string(),
            request_body: String::from_utf8_lossy(&request.body).into_owned(),
            request_body_file: String::new(),
            request_body_file_guard: None,
            request_body_prepared: false,
            request_length: request_length(request).to_string(),
            upstream_status: String::new(),
            upstream_cache_status: String::new(),
            proxy_cache_background: false,
            upstream_response_time: String::new(),
            upstream_headers: Vec::new(),
            upstream_trailers: Vec::new(),
            sent_headers: Vec::new(),
            sent_trailers: Vec::new(),
            sent_connection: String::new(),
            sent_keep_alive: String::new(),
            sent_transfer_encoding: String::new(),
            response_from_upstream: false,
            client_abort: None,
            error_page: false,
            started: Instant::now(),
            limit_req_status: String::new(),
            limit_conn_status: String::new(),
            invalid_referer: String::new(),
            secure_link: String::new(),
            secure_link_expires: String::new(),
            msie: String::new(),
            modern_browser: String::new(),
            ancient_browser: String::new(),
            userid: UserIdState::default(),
            document_root: PathBuf::new(),
            request_filename: PathBuf::new(),
            proxy_host: String::new(),
            proxy_port: String::new(),
            upstream_addr: String::new(),
            headers: request.headers.clone(),
            captures: Vec::new(),
            server_names: server.names.clone(),
            custom: HashMap::new(),
            maps: server.context.maps.clone(),
            geos: server.context.geos.clone(),
            split_clients: server.context.split_clients.clone(),
            limits: Arc::new(Mutex::new(LimitState::default())),
            stats: Arc::new(ConnectionStats::default()),
            upstream_keepalive: Arc::new(UpstreamKeepalivePool::default()),
            upstream_keepalive_generation: 0,
            proxy_cache: Arc::new(ProxyCacheRegistry::default()),
            client_tls_cache: Arc::new(ClientTlsConfigCache::default()),
            error_logs: None,
            rewrite_logger: None,
            rewrite_log: false,
            uninitialized_variable_warn: false,
        }
    }

    pub(super) fn error_logger_for(&self, context: &EffectiveConfig) -> Option<Arc<Logger>> {
        self.error_logs
            .as_ref()
            .map(|logs| logs.for_context(context))
    }

    pub(super) fn set_variable_warning_context(
        &mut self,
        context: &EffectiveConfig,
        logger: Option<Arc<Logger>>,
    ) {
        self.rewrite_logger = logger;
        self.uninitialized_variable_warn = context.uninitialized_variable_warn;
    }

    pub(super) fn rewrite_log_enabled(&self) -> bool {
        self.rewrite_log
            || self
                .rewrite_logger
                .as_ref()
                .is_some_and(|logger| logger.enabled(LogLevel::Debug))
    }
}

pub(super) fn server_name_value(name: &ServerName) -> String {
    match name {
        ServerName::Exact(value)
        | ServerName::LeadingWildcard(value)
        | ServerName::TrailingWildcard(value) => value.clone(),
        ServerName::Regex(regex) => regex.as_str().to_string(),
    }
}

mod captures;
mod eval;
mod headers;
mod maps;
mod proxy_protocol;

pub(super) use captures::{expand_captures, expand_captures_in_args, set_regex_captures};
pub(super) use eval::{
    binary_address_bytes, eval_variables, eval_variables_bytes, eval_variables_with_stack,
    variable_value,
};
pub(super) use headers::response_header_value;
pub(super) use maps::{
    geo_value, map_value, nginx_murmur_hash2, parse_geo_address, split_clients_value,
};
pub(super) use proxy_protocol::proxy_protocol_tlv_value;
