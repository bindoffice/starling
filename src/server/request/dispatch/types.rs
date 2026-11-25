use super::*;

pub(in crate::server) enum DispatchAction {
    Response(DispatchOutcome),
    Tunnel(TunnelDispatch),
}

pub(in crate::server) struct DispatchOutcome {
    pub(in crate::server) response: Response,
    pub(in crate::server) response_prepared: bool,
    pub(in crate::server) response_chunked: bool,
    pub(in crate::server) error_logger: Arc<Logger>,
    pub(in crate::server) keepalive_timeout: Duration,
    pub(in crate::server) keepalive_min_timeout: Duration,
    pub(in crate::server) keep_alive: bool,
    pub(in crate::server) keepalive_header: Option<u64>,
    pub(in crate::server) send_timeout: Duration,
    pub(in crate::server) tcp_nodelay: bool,
    pub(in crate::server) limit_rate: ResponseRate,
    pub(in crate::server) server_tokens: ServerTokens,
    pub(in crate::server) chunked_transfer_encoding: bool,
    pub(in crate::server) http2_chunk_size: usize,
    pub(in crate::server) close: ConnectionCloseConfig,
    pub(in crate::server) _body_file_guard: Option<Arc<RequestBodyFile>>,
    pub(in crate::server) _connection_guard: Option<ConnectionLimitGuard>,
    pub(in crate::server) deferred_access_log: Option<Box<DeferredAccessLog>>,
    pub(in crate::server) upgrade: Option<Box<ProxyUpgrade>>,
}

pub(in crate::server) struct DeferredAccessLog {
    pub(super) formats: HashMap<String, crate::config::AccessLogFormat>,
    pub(super) context: Arc<EffectiveConfig>,
    pub(super) variables: Variables,
}

impl DeferredAccessLog {
    pub(in crate::server) fn write(mut self) {
        if !self.variables.upstream_response_time.is_empty() {
            self.variables.upstream_response_time =
                format!("{:.3}", self.variables.started.elapsed().as_secs_f64());
        }
        log_access_with_formats(&self.formats, &self.context, &self.variables);
    }

    pub(in crate::server) fn write_upgraded(self, bytes_sent: u64, body_bytes_sent: u64) {
        let mut updated = self;
        updated.variables.bytes_sent = bytes_sent;
        updated.variables.body_bytes_sent = body_bytes_sent;
        updated.write();
    }
}

pub(in crate::server) struct DispatchRequestContext<'a> {
    pub(in crate::server) request: &'a mut Request,
    pub(in crate::server) connection: ConnectionAddresses,
    pub(in crate::server) connection_request: ConnectionRequestState,
    pub(in crate::server) invalid_host: bool,
    pub(in crate::server) client_abort: Option<ClientAbortState>,
    pub(in crate::server) request_length: Option<u64>,
}
