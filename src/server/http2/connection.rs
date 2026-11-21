use super::*;

#[derive(Clone, Copy)]
pub(super) struct Http2ConnectionSettings {
    pub(super) limits: Http2Limits,
    pub(super) max_header_field_size: usize,
    pub(super) header_timeout: Duration,
    pub(super) keepalive_timeout: Duration,
    pub(super) keepalive_time: Duration,
    pub(super) keepalive_requests: u64,
}

impl Default for Http2ConnectionSettings {
    fn default() -> Self {
        Self {
            limits: Http2Limits::default(),
            max_header_field_size: 8 * 1024,
            header_timeout: Duration::from_secs(60),
            keepalive_timeout: Duration::from_secs(75),
            keepalive_time: Duration::from_secs(3600),
            keepalive_requests: 1000,
        }
    }
}

impl Http2ConnectionSettings {
    pub(super) fn idle_timeout(self, requests: u64) -> Duration {
        if requests == 0 {
            self.header_timeout
        } else {
            self.keepalive_timeout
        }
    }

    pub(super) fn should_graceful_shutdown(self, requests: u64, started: Instant) -> bool {
        requests > 0
            && (self.keepalive_timeout == Duration::ZERO
                || requests >= self.keepalive_requests
                || (self.keepalive_time != Duration::ZERO
                    && started.elapsed() >= self.keepalive_time))
    }
}

pub(super) async fn serve_connection<S>(
    mut connection: h2_server::Connection<S, Bytes>,
    settings: Http2ConnectionSettings,
    addresses: ConnectionAddresses,
    group: ListenerGroup,
    state: SharedState,
    nodelay: Option<ClientSocketControl>,
    mut shutdown: Option<watch::Receiver<ShutdownMode>>,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let in_flight = Arc::new(AtomicU64::new(0));
    let connection_stats = Arc::new(Mutex::new(state.stats.connection_started()));
    let mut streams = JoinSet::new();
    let (connection_fault, mut connection_faults) = H2ConnectionFault::channel();
    let connection_started = Instant::now();
    let mut accepted_requests = 0_u64;
    let mut shutting_down = false;
    let mut fatal_shutdown = false;
    let shutdown_enabled = shutdown.is_some();

    loop {
        if let Some(mode) = requested_shutdown_mode(&shutdown) {
            match mode {
                ShutdownMode::Graceful if !shutting_down => {
                    connection.graceful_shutdown();
                    shutting_down = true;
                }
                ShutdownMode::Fast => {
                    connection.abrupt_shutdown(h2::Reason::NO_ERROR);
                    fatal_shutdown = true;
                    break;
                }
                ShutdownMode::Running | ShutdownMode::Graceful => {}
            }
        }
        if !shutting_down
            && settings.should_graceful_shutdown(accepted_requests, connection_started)
        {
            connection.graceful_shutdown();
            shutting_down = true;
        }
        if shutting_down && in_flight.load(Ordering::Relaxed) == 0 && !connection.has_streams() {
            // Poll the driver once so the queued GOAWAY is flushed before the
            // connection future is dropped.
            let _ = tokio::time::timeout(Duration::from_secs(1), connection.accept()).await;
            break;
        }

        let idle = !shutting_down && in_flight.load(Ordering::Relaxed) == 0;
        let idle_timeout = settings.idle_timeout(accepted_requests);
        let accepted = tokio::select! {
            accepted = connection.accept() => accepted,
            joined = streams.join_next(), if !streams.is_empty() => {
                if let Some(Err(err)) = joined {
                    state
                        .logger
                        .log(LogLevel::Error, format!("HTTP/2 stream task failed: {err}"));
                }
                continue;
            }
            mode = wait_for_optional_shutdown(&mut shutdown), if shutdown_enabled && !shutting_down =>
            {
                match mode {
                    ShutdownMode::Graceful if !shutting_down => {
                        connection.graceful_shutdown();
                        shutting_down = true;
                    }
                    ShutdownMode::Fast => {
                        connection.abrupt_shutdown(h2::Reason::NO_ERROR);
                        fatal_shutdown = true;
                        break;
                    }
                    ShutdownMode::Running | ShutdownMode::Graceful => {}
                }
                continue;
            }
            fault = connection_faults.recv(), if !fatal_shutdown => {
                if let Some(reason) = fault {
                    state.logger.log(
                        LogLevel::Info,
                        format!("HTTP/2 connection protocol error: {reason:?}"),
                    );
                    connection.abrupt_shutdown(reason);
                    fatal_shutdown = true;
                }
                break;
            }
            _ = tokio::time::sleep(idle_timeout), if idle && idle_timeout != Duration::ZERO => {
                state.logger.log(
                    LogLevel::Info,
                    format!("HTTP/2 connection from {} timed out", addresses.remote),
                );
                connection.graceful_shutdown();
                break;
            }
        };
        let Some(accepted) = accepted else {
            break;
        };
        if shutting_down {
            if let Ok((_request, mut respond)) = accepted {
                respond.send_reset(h2::Reason::REFUSED_STREAM);
            }
            continue;
        }
        let (request, respond) = match accepted {
            Ok(request) => request,
            Err(err) => {
                state
                    .logger
                    .log(LogLevel::Info, format!("HTTP/2 connection error: {err}"));
                break;
            }
        };

        state.stats.requests.fetch_add(1, Ordering::Relaxed);
        accepted_requests = accepted_requests.saturating_add(1);
        if in_flight.fetch_add(1, Ordering::Relaxed) == 0
            && let Ok(mut guard) = connection_stats.lock()
        {
            guard.set_writing();
        }
        let state = state.clone();
        let group = group.clone();
        let addresses = addresses.clone();
        let in_flight = in_flight.clone();
        let connection_stats = connection_stats.clone();
        let options = super::H2RequestOptions {
            max_header_field_size: settings.max_header_field_size,
            max_header_list_size: settings.limits.max_header_list_size as usize,
            nodelay: nodelay.clone(),
        };
        let connection_fault = connection_fault.clone();
        streams.spawn(async move {
            let remote = addresses.remote.clone();
            if let Err(err) = super::handle_h2_request(
                request,
                respond,
                &group,
                addresses,
                &state,
                options,
                Some(connection_fault),
            )
            .await
            {
                state.logger.log(
                    LogLevel::Info,
                    format!("HTTP/2 request from {remote} failed: {err}"),
                );
            }
            if in_flight.fetch_sub(1, Ordering::Relaxed) == 1
                && let Ok(mut guard) = connection_stats.lock()
            {
                guard.set_waiting();
            }
        });
        if !shutting_down
            && settings.should_graceful_shutdown(accepted_requests, connection_started)
        {
            connection.graceful_shutdown();
            shutting_down = true;
        }
    }

    if fatal_shutdown {
        streams.shutdown().await;
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            while connection.accept().await.is_some() {}
        })
        .await;
    } else {
        while let Some(result) = streams.join_next().await {
            if let Err(err) = result {
                state
                    .logger
                    .log(LogLevel::Error, format!("HTTP/2 stream task failed: {err}"));
            }
        }
    }
}

pub(super) fn settings_for_connection(
    config: &RuntimeConfig,
    group: &ListenerGroup,
    connection: &ConnectionAddresses,
) -> Http2ConnectionSettings {
    let Some(server) = http2_server_for_connection(config, group, connection) else {
        return Http2ConnectionSettings::default();
    };
    let mut limits = server.context.http2_limits;
    limits.max_header_list_size = server
        .context
        .large_client_header_buffers
        .number
        .saturating_mul(server.context.large_client_header_buffers.size)
        .min(u32::MAX as usize) as u32;
    Http2ConnectionSettings {
        limits,
        max_header_field_size: server.context.large_client_header_buffers.size,
        header_timeout: server.context.client_header_timeout,
        keepalive_timeout: server.context.keepalive_timeout,
        keepalive_time: server.context.keepalive_time,
        keepalive_requests: server.context.keepalive_requests,
    }
}

pub(super) fn http2_server_for_connection<'a>(
    config: &'a RuntimeConfig,
    group: &ListenerGroup,
    connection: &ConnectionAddresses,
) -> Option<&'a ServerConfig> {
    let http = config.http.as_ref()?;
    let active_group = build_listener_groups(config)
        .into_iter()
        .find(|candidate| candidate.address == group.address)
        .unwrap_or_else(|| group.clone());
    Some(
        connection
            .tls
            .as_ref()
            .map(|tls| tls.server_name.as_str())
            .filter(|name| !name.is_empty())
            .map(|name| select_server(http, &active_group, name))
            .unwrap_or_else(|| &http.servers[active_group.default_server_id]),
    )
}
