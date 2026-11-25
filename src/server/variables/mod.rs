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
    pub(super) server_name: Arc<str>,
    pub(super) remote_addr: Arc<str>,
    pub(super) remote_user: String,
    pub(super) remote_port: Arc<str>,
    pub(super) server_addr: Arc<str>,
    pub(super) server_port: Arc<str>,
    pub(super) scheme: &'static str,
    pub(super) https: &'static str,
    pub(super) http2: &'static str,
    pub(super) pipelined: bool,
    pub(super) connection_id: Option<u64>,
    pub(super) served_requests: Option<u64>,
    pub(super) connection_started: Option<Instant>,
    pub(super) hostname: Arc<str>,
    pub(super) request_id: Arc<OnceLock<String>>,
    pub(super) realip_remote_addr: Option<Arc<str>>,
    pub(super) realip_remote_port: Option<Arc<str>>,
    pub(super) uri: String,
    pub(super) fastcgi_script_name: String,
    pub(super) fastcgi_path_info: String,
    pub(super) uwsgi_script_name: String,
    pub(super) scgi_script_name: String,
    pub(super) args: String,
    pub(super) status: u16,
    pub(super) bytes_sent: u64,
    pub(super) body_bytes_sent: u64,
    pub(super) content_length: String,
    pub(super) content_type: String,
    pub(super) request_body: String,
    pub(super) request_body_file: String,
    pub(super) request_body_file_guard: Option<Arc<RequestBodyFile>>,
    pub(super) request_body_prepared: bool,
    pub(super) request_length: Option<u64>,
    pub(super) client_abort: Option<ClientAbortState>,
    pub(super) started: Instant,
    pub(super) document_root: PathBuf,
    pub(super) request_filename: PathBuf,
    pub(super) proxy_host: String,
    pub(super) proxy_port: String,
    pub(super) upstream_addr: String,
    pub(super) headers: Arc<Vec<Header>>,
    pub(super) captures: Vec<String>,
    pub(super) server_names: Arc<[ServerName]>,
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
    pub(super) content_handler: Option<ContentHandlerKind>,
    pub(super) msie: String,
    pub(super) modern_browser: String,
    pub(super) ancient_browser: String,
    pub(super) cold: VariablesCold,
}

pub(in crate::server) struct VariablesShared {
    pub(in crate::server) limits: Arc<Mutex<LimitState>>,
    pub(in crate::server) stats: Arc<ConnectionStats>,
    pub(in crate::server) upstream_keepalive: Arc<UpstreamKeepalivePool>,
    pub(in crate::server) upstream_keepalive_generation: u64,
    pub(in crate::server) proxy_cache: Arc<ProxyCacheRegistry>,
    pub(in crate::server) client_tls_cache: Arc<ClientTlsConfigCache>,
}

impl Default for VariablesShared {
    fn default() -> Self {
        Self {
            limits: Arc::new(Mutex::new(LimitState::default())),
            stats: Arc::new(ConnectionStats::default()),
            upstream_keepalive: Arc::new(UpstreamKeepalivePool::default()),
            upstream_keepalive_generation: 0,
            proxy_cache: Arc::new(ProxyCacheRegistry::default()),
            client_tls_cache: Arc::new(ClientTlsConfigCache::default()),
        }
    }
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
    #[cfg(test)]
    pub(super) fn new<L: AddressVariables + ?Sized, R: AddressVariables + ?Sized>(
        request: &Request,
        host: &str,
        server: &ServerConfig,
        local: &L,
        remote: &R,
    ) -> Self {
        let mut variables = Self::new_with_shared(
            request,
            host.to_owned(),
            server,
            local,
            remote,
            VariablesShared::default(),
            None,
            false,
            None,
        );
        let (path, query) = request.path_and_query();
        variables.uri = path.to_string();
        variables.args = query.unwrap_or_default().to_string();
        variables.realip_remote_addr = Some(Arc::from(remote.variable_address()));
        variables.realip_remote_port = Some(Arc::from(remote.variable_port()));
        variables.request_length = Some(request_length(request));
        variables
    }

    pub(in crate::server) fn new_for_dispatch(
        request: &Request,
        host: String,
        server: &ServerConfig,
        connection: &ConnectionAddresses,
        shared: VariablesShared,
        request_length: Option<u64>,
    ) -> Self {
        Self::new_with_shared(
            request,
            host,
            server,
            &connection.local,
            &connection.remote,
            shared,
            request_length,
            true,
            Some(connection),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_shared<L: AddressVariables + ?Sized, R: AddressVariables + ?Sized>(
        request: &Request,
        host: String,
        server: &ServerConfig,
        local: &L,
        remote: &R,
        shared: VariablesShared,
        request_length: Option<u64>,
        dispatch: bool,
        addresses: Option<&ConnectionAddresses>,
    ) -> Self {
        let (path, query) = request.path_and_query();
        let path_variables = !dispatch;
        let remote_addr = addresses
            .map(|addresses| addresses.remote_addr_text.clone())
            .unwrap_or_else(|| Arc::from(remote.variable_address()));
        let remote_port = addresses
            .map(|addresses| addresses.remote_port_text.clone())
            .unwrap_or_else(|| Arc::from(remote.variable_port()));
        let server_addr = addresses
            .map(|addresses| addresses.local_addr_text.clone())
            .unwrap_or_else(|| Arc::from(local.variable_address()));
        let server_port = addresses
            .map(|addresses| addresses.local_port_text.clone())
            .unwrap_or_else(|| Arc::from(local.variable_port()));
        Self {
            method: request.method.clone(),
            request_target: request.target.clone(),
            version: request.version.clone(),
            host,
            server_name: server.default_server_name.clone(),
            remote_addr,
            remote_user: request
                .header("authorization")
                .and_then(parse_authorization_header)
                .map(|credentials| credentials.username)
                .unwrap_or_default(),
            remote_port,
            server_addr,
            server_port,
            scheme: "http",
            https: "",
            http2: "",
            pipelined: false,
            connection_id: None,
            served_requests: None,
            connection_started: None,
            hostname: hostname(),
            request_id: Arc::new(OnceLock::new()),
            realip_remote_addr: (!dispatch).then(|| Arc::from(remote.variable_address())),
            realip_remote_port: (!dispatch).then(|| Arc::from(remote.variable_port())),
            uri: if path_variables {
                path.to_string()
            } else {
                String::new()
            },
            fastcgi_script_name: if path_variables {
                path.to_string()
            } else {
                String::new()
            },
            fastcgi_path_info: String::new(),
            uwsgi_script_name: if path_variables {
                path.to_string()
            } else {
                String::new()
            },
            scgi_script_name: if path_variables {
                path.to_string()
            } else {
                String::new()
            },
            args: if path_variables {
                query.unwrap_or_default().to_string()
            } else {
                String::new()
            },
            status: 0,
            bytes_sent: 0,
            body_bytes_sent: 0,
            content_length: request
                .header("content-length")
                .unwrap_or_default()
                .to_string(),
            content_type: request
                .headers("content-type")
                .collect::<Vec<_>>()
                .join(", "),
            request_body: String::from_utf8_lossy(&request.body).into_owned(),
            request_body_file: String::new(),
            request_body_file_guard: None,
            request_body_prepared: false,
            request_length,
            client_abort: None,
            started: Instant::now(),
            document_root: PathBuf::new(),
            request_filename: PathBuf::new(),
            proxy_host: String::new(),
            proxy_port: String::new(),
            upstream_addr: String::new(),
            headers: request.headers.clone(),
            captures: Vec::new(),
            server_names: server.names.clone(),
            maps: server.context.maps.clone(),
            geos: server.context.geos.clone(),
            split_clients: server.context.split_clients.clone(),
            limits: shared.limits,
            stats: shared.stats,
            upstream_keepalive: shared.upstream_keepalive,
            upstream_keepalive_generation: shared.upstream_keepalive_generation,
            proxy_cache: shared.proxy_cache,
            client_tls_cache: shared.client_tls_cache,
            error_logs: None,
            rewrite_logger: None,
            rewrite_log: false,
            uninitialized_variable_warn: false,
            content_handler: None,
            msie: String::new(),
            modern_browser: String::new(),
            ancient_browser: String::new(),
            cold: VariablesCold::default(),
        }
    }

    #[inline]
    pub(super) fn cold(&self) -> &VariablesCold {
        &self.cold
    }

    #[inline]
    pub(super) fn cold_mut(&mut self) -> &mut VariablesCold {
        &mut self.cold
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

    pub(super) fn request_id_value(&self) -> &str {
        self.request_id.get_or_init(next_request_id)
    }

    pub(super) fn pipe_value(&self) -> &'static str {
        if self.pipelined { "p" } else { "." }
    }

    pub(super) fn connection_value(&self) -> String {
        self.connection_id
            .map(|value| value.to_string())
            .unwrap_or_default()
    }

    pub(super) fn connection_requests_value(&self) -> String {
        self.served_requests
            .map(|value| value.to_string())
            .unwrap_or_default()
    }

    pub(super) fn connection_time_value(&self) -> String {
        self.connection_started
            .map(|started| format!("{:.3}", started.elapsed().as_secs_f64()))
            .unwrap_or_default()
    }
}

impl std::ops::Deref for Variables {
    type Target = VariablesCold;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.cold()
    }
}

impl std::ops::DerefMut for Variables {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.cold_mut()
    }
}

pub(super) fn server_name_value(name: &ServerName) -> String {
    name.display().to_string()
}

mod captures;
mod cold;
mod eval;
mod headers;
mod maps;
mod proxy_protocol;

pub(super) use captures::{
    clear_regex_captures, expand_captures, expand_captures_in_args, set_regex_captures,
    set_server_name_captures,
};
use cold::VariablesCold;
pub(super) use eval::{
    binary_address_bytes, eval_variables, eval_variables_bytes, eval_variables_with_stack,
    variable_bytes_for_log, variable_value,
};
pub(super) use headers::response_header_value;
pub(super) use maps::{
    geo_value, map_value, nginx_murmur_hash2, parse_geo_address, split_clients_value,
};
pub(super) use proxy_protocol::proxy_protocol_tlv_value;
