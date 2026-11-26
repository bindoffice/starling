use super::*;

const LINGERING_CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const SHUTDOWN_GRACE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
pub(super) struct Http2ConnectionSettings {
    pub(super) limits: Http2Limits,
    pub(super) max_header_field_size: usize,
    pub(super) max_frame_size: u32,
    pub(super) header_timeout: Duration,
    pub(super) body_timeout: Duration,
    pub(super) keepalive_timeout: Duration,
    pub(super) keepalive_time: Duration,
    pub(super) keepalive_requests: u64,
}

impl Default for Http2ConnectionSettings {
    fn default() -> Self {
        Self {
            limits: Http2Limits::default(),
            max_header_field_size: 8 * 1024,
            max_frame_size: 16 * 1024,
            header_timeout: Duration::from_secs(60),
            body_timeout: Duration::from_secs(60),
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

pub(super) struct H2ConnectionContext {
    pub(super) settings: Http2ConnectionSettings,
    pub(super) addresses: ConnectionAddresses,
    pub(super) group: ListenerGroup,
    pub(super) state: SharedState,
    pub(super) nodelay: Option<ClientSocketControl>,
    pub(super) sendfile_supported: bool,
    pub(super) shutdown: Option<watch::Receiver<ShutdownMode>>,
    pub(super) frame_timeouts: FrameTimeoutTracker,
    pub(super) frame_timeout_events: FrameTimeoutEvents,
    pub(super) settings_acked: Arc<AtomicBool>,
}

pub(super) async fn serve_connection<S>(
    mut connection: h2_server::Connection<S, Bytes>,
    context: H2ConnectionContext,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let H2ConnectionContext {
        settings,
        addresses,
        group,
        state,
        nodelay,
        sendfile_supported,
        mut shutdown,
        frame_timeouts,
        mut frame_timeout_events,
        settings_acked,
    } = context;
    let in_flight = Arc::new(AtomicU64::new(0));
    let connection_stats = Arc::new(Mutex::new(state.stats.connection_started()));
    let mut streams = JoinSet::new();
    let (connection_fault, mut connection_faults) = H2ConnectionFault::channel();
    let connection_started = Instant::now();
    let mut accepted_requests = 0_u64;
    let mut draining = false;
    let mut shutting_down = false;
    let mut goaway_sent = false;
    let mut linger_until = None;
    let mut shutdown_until = None;
    let mut fatal_shutdown = false;
    let mut reload = state.reload_receiver();
    let shutdown_enabled = shutdown.is_some();

    loop {
        if let Some(mode) = requested_shutdown_mode(&shutdown) {
            match mode {
                ShutdownMode::Graceful if !draining && !shutting_down => {
                    draining = true;
                    shutdown_until.get_or_insert(Instant::now() + SHUTDOWN_GRACE_TIMEOUT);
                    if !goaway_sent {
                        connection.go_away_and_linger();
                        goaway_sent = true;
                    }
                }
                ShutdownMode::Fast => {
                    connection.abrupt_shutdown(h2::Reason::NO_ERROR);
                    fatal_shutdown = true;
                    break;
                }
                ShutdownMode::Running | ShutdownMode::Graceful => {}
            }
        }
        if shutdown_until.is_some_and(|deadline| Instant::now() >= deadline) {
            connection.go_away_after_flush(h2::Reason::NO_ERROR);
            fatal_shutdown = true;
            break;
        }
        if draining
            && !shutting_down
            && in_flight.load(Ordering::Relaxed) == 0
            && !connection.has_streams_or_other_references()
        {
            if !goaway_sent {
                connection.go_away_and_linger();
                goaway_sent = true;
            }
            shutting_down = true;
            linger_until = Some(Instant::now() + LINGERING_CLOSE_TIMEOUT);
            continue;
        }
        if let Some(deadline) = linger_until
            && Instant::now() >= deadline
        {
            linger_until = None;
            connection.go_away_after_flush(h2::Reason::NO_ERROR);
            continue;
        }
        if shutting_down
            && linger_until.is_none()
            && in_flight.load(Ordering::Relaxed) == 0
            && !connection.has_streams_or_other_references()
        {
            // Keep driving the connection until h2 has flushed both the
            // graceful GOAWAY and any DATA still buffered by closed streams.
            let _ = tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(result) = connection.accept().await {
                    if result.is_err() {
                        break;
                    }
                }
            })
            .await;
            break;
        }

        let idle = !draining
            && !shutting_down
            && in_flight.load(Ordering::Relaxed) == 0
            && !frame_timeouts.has_pending();
        let idle_timeout = settings.idle_timeout(accepted_requests);
        let frame_timeout = frame_timeouts.next_deadline();
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
            _ = frame_timeout_events.changed() => {
                continue;
            }
            _ = async move {
                match frame_timeout {
                    Some(deadline) => {
                        tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
                    }
                    None => std::future::pending::<()>().await,
                }
            } => {
                if let Some(frame) = frame_timeouts.take_expired(Instant::now()) {
                    if frame.headers {
                        // The HEADERS block may not have produced an h2 stream
                        // yet, so queue the wire-level reset directly.
                        connection.queue_raw_reset(frame.stream_id, h2::Reason::PROTOCOL_ERROR);
                    } else {
                        connection.reset_stream(frame.stream_id, h2::Reason::PROTOCOL_ERROR);
                    }
                }
                continue;
            }
            _ = async move {
                match linger_until {
                    Some(deadline) => {
                        tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
                    }
                    None => std::future::pending::<()>().await,
                }
            } => {
                linger_until = None;
                connection.go_away_after_flush(h2::Reason::NO_ERROR);
                continue;
            }
            mode = wait_for_optional_shutdown(&mut shutdown),
                if shutdown_enabled && !draining && !shutting_down =>
            {
                match mode {
                    ShutdownMode::Graceful if !draining && !shutting_down => {
                        draining = true;
                        shutdown_until.get_or_insert(Instant::now() + SHUTDOWN_GRACE_TIMEOUT);
                        if !goaway_sent {
                            connection.go_away_and_linger();
                            goaway_sent = true;
                        }
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
            changed = reload.changed(), if !draining && !shutting_down => {
                let _ = changed;
                draining = true;
                if !goaway_sent {
                    connection.go_away_and_linger();
                    goaway_sent = true;
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
                if accepted_requests == 0 {
                    connection.abrupt_shutdown(h2::Reason::PROTOCOL_ERROR);
                    fatal_shutdown = true;
                    break;
                } else {
                    draining = true;
                }
                continue;
            }
        };
        let Some(accepted) = accepted else {
            break;
        };
        if draining || shutting_down {
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
                fatal_shutdown = true;
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
            connection_id: connection_stats
                .lock()
                .map(|guard| guard.id())
                .unwrap_or_default(),
            connection_started,
            served_requests: accepted_requests,
            body_preread_size: settings.limits.body_preread_size,
            settings_acked: settings_acked.clone(),
            sendfile_supported,
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
        if !draining
            && !shutting_down
            && settings.should_graceful_shutdown(accepted_requests, connection_started)
        {
            draining = true;
            if !goaway_sent {
                connection.go_away_and_linger();
                goaway_sent = true;
            }
        }
    }

    if fatal_shutdown {
        streams.shutdown().await;
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            while let Some(result) = connection.accept().await {
                if result.is_err() {
                    break;
                }
            }
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
        // Nginx accepts DATA frames up to the protocol maximum instead of
        // tying the frame size to the decoded header-list limit.
        max_frame_size: (1 << 24) - 1,
        header_timeout: server.context.client_header_timeout,
        body_timeout: server.context.client_body_timeout,
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
