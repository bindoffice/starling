use super::*;

pub(in crate::server) async fn stream_udp_loop(
    listener: UdpSocket,
    route: StreamRoute,
    state: SharedState,
    mut shutdown: watch::Receiver<ShutdownMode>,
) {
    let listener = Arc::new(listener);
    let sessions: Arc<Mutex<HashMap<SocketAddr, UdpSession>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let mut session_tasks = JoinSet::new();
    let next_session_id = AtomicU64::new(0);
    let shutdown_mode = loop {
        let mut datagram = vec![0_u8; u16::MAX as usize];
        tokio::select! {
            changed = shutdown.changed() => {
                let mode = if changed.is_err() {
                    ShutdownMode::Fast
                } else {
                    *shutdown.borrow_and_update()
                };
                if mode.requested() {
                    break mode;
                }
            }
            joined = session_tasks.join_next(), if !session_tasks.is_empty() => {
                if let Some(Err(err)) = joined {
                    state.logger.log(
                        LogLevel::Error,
                        format!("stream UDP session task failed: {err}"),
                    );
                }
            }
            received = listener.recv_from(&mut datagram) => {
                match received {
                    Ok((size, remote)) => {
                        datagram.truncate(size);
                        let listener = listener.clone();
                        let route = route.clone();
                        let state = state.clone();
                        let sessions = sessions.clone();
                        let session_id = next_session_id.fetch_add(1, Ordering::Relaxed);
                        let (sender, receiver) = mpsc::channel(64);
                        {
                            let mut active = sessions.lock().unwrap();
                            if let Some(session) = active.get(&remote) {
                                match session.sender.try_send(datagram.clone()) {
                                    Ok(()) => continue,
                                    Err(mpsc::error::TrySendError::Full(_)) => {
                                        state.logger.log(
                                            LogLevel::Warn,
                                            format!(
                                                "stream UDP session queue for {remote} is full; dropping datagram"
                                            ),
                                        );
                                        continue;
                                    }
                                    Err(mpsc::error::TrySendError::Closed(_)) => {}
                                }
                            }
                            if sender.try_send(datagram).is_err() {
                                state.logger.log(
                                    LogLevel::Error,
                                    format!("cannot initialize stream UDP session for {remote}"),
                                );
                                continue;
                            }
                            active.insert(
                                remote,
                                UdpSession {
                                    id: session_id,
                                    sender,
                                },
                            );
                        }
                        let shutdown = shutdown.clone();
                        session_tasks.spawn(async move {
                            handle_stream_udp_session(
                                listener,
                                receiver,
                                remote,
                                route,
                                state,
                                shutdown,
                            )
                            .await;
                            let mut active = sessions.lock().unwrap();
                            if active
                                .get(&remote)
                                .is_some_and(|session| session.id == session_id)
                            {
                                active.remove(&remote);
                            }
                        });
                    }
                    Err(err) => {
                        state.logger.log(
                            LogLevel::Error,
                            format!("stream UDP recv_from() failed: {err}"),
                        );
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }
    };
    finish_connection_tasks(session_tasks, shutdown_mode, shutdown).await;
}

pub(in crate::server) struct UdpSession {
    id: u64,
    sender: mpsc::Sender<Vec<u8>>,
}

pub(in crate::server) async fn handle_stream_udp_session(
    listener: Arc<UdpSocket>,
    mut datagrams: mpsc::Receiver<Vec<u8>>,
    remote: SocketAddr,
    route: StreamRoute,
    state: SharedState,
    mut shutdown: watch::Receiver<ShutdownMode>,
) {
    let config = state.config.read().await.clone();
    let Some(stream) = config.stream.as_ref() else {
        return;
    };
    let Some(server) = stream_server_for_route(stream, &route) else {
        return;
    };
    let logger = stream_logger(&state, server).await;
    let mut log = StreamLogContext {
        logger,
        started: Instant::now(),
        remote: ListenAddress::Inet(remote),
        realip_remote: ListenAddress::Inet(remote),
        local: route.address.clone(),
        proxy_protocol_addr: None,
        proxy_protocol_port: None,
        proxy_protocol_server_addr: None,
        proxy_protocol_server_port: None,
        proxy_protocol_tlvs: None,
        protocol: "UDP",
        server_name: server.server_name.clone(),
        limit_conn_status: String::new(),
        status: 0,
        bytes_sent: 0,
        bytes_received: 0,
        upstream_addr: String::new(),
        upstream_bytes_sent: 0,
        upstream_bytes_received: 0,
        upstream_connect_time: None,
        upstream_first_byte_time: None,
        upstream_session_time: None,
        tls: None,
        ssl_preread: None,
        variables: HashMap::new(),
        maps: stream.maps.clone(),
        geos: stream.geos.clone(),
        split_clients: stream.split_clients.clone(),
    };
    apply_stream_sets(&server.sets, &mut log);
    let remote_address = ListenAddress::Inet(remote);
    let Ok(_limit_guard) = acquire_stream_connection_limits(server, &state, &mut log) else {
        log_stream_access(&config, server, &log);
        return;
    };
    if reject_stream_access(server, &remote_address, &mut log) {
        log_stream_access(&config, server, &log);
        return;
    }
    if server.handler == StreamHandler::Return {
        if let Some(template) = &server.return_value {
            let response = eval_stream_log_variables(template, &log);
            log.status = 200;
            if !response.is_empty() {
                match listener.send_to(response.as_bytes(), remote).await {
                    Ok(size) => log.bytes_sent = size as u64,
                    Err(err) => {
                        log.status = 500;
                        log.logger.log(
                            LogLevel::Info,
                            format!("stream return send_to() {remote} failed: {err}"),
                        );
                    }
                }
            }
        }
        log_stream_access(&config, server, &log);
        return;
    }
    if server.handler == StreamHandler::Pass {
        log.status = 500;
        log.logger
            .log(LogLevel::Error, "cannot pass udp connection");
        log_stream_access(&config, server, &log);
        return;
    }
    let Some(proxy) = &server.proxy_pass else {
        log.status = 502;
        log_stream_access(&config, server, &log);
        return;
    };
    let peer = remote.ip();
    let hash_key = peer.to_string();
    let Some((target, mut guard)) = resolve_stream_target(
        &config,
        proxy.upstream.as_deref(),
        &proxy.target,
        Some(peer),
        &hash_key,
    ) else {
        log.status = 502;
        log.logger.log(
            LogLevel::Error,
            format!("invalid stream UDP upstream \"{}\"", proxy.target),
        );
        log_stream_access(&config, server, &log);
        return;
    };
    let attempt_started = Instant::now();
    let host = target.host.as_str();
    let port = target.port;
    log.upstream_addr = upstream_target_display(&target);
    let local_bind = evaluate_stream_proxy_bind(server, &log);
    let upstream_address = match tokio::net::lookup_host((host, port)).await {
        Ok(mut addresses) => match addresses.next() {
            Some(address) => address,
            None => {
                log.status = 502;
                log.logger.log(
                    LogLevel::Error,
                    format!("stream UDP resolve() for {host}:{port} returned no addresses"),
                );
                log_stream_access(&config, server, &log);
                return;
            }
        },
        Err(err) => {
            log.status = 502;
            log.logger.log(
                LogLevel::Error,
                format!("stream UDP resolve() for {host}:{port} failed: {err}"),
            );
            log_stream_access(&config, server, &log);
            return;
        }
    };
    let upstream =
        match bind_stream_udp_socket(local_bind.as_deref(), upstream_address.is_ipv4()).await {
            Ok(socket) => socket,
            Err(err) => {
                log.status = 502;
                log.logger.log(
                    LogLevel::Error,
                    format!("stream UDP bind() for upstream failed: {err}"),
                );
                log_stream_access(&config, server, &log);
                return;
            }
        };
    let socket_options = StreamSocketOptions::from_server(server);
    if let Err(err) = apply_stream_socket_options(&upstream, socket_options) {
        log.status = 502;
        if let Some(guard) = guard.as_mut() {
            guard.mark_failed();
        }
        log.logger.log(
            LogLevel::Error,
            format!("setsockopt() for UDP {host}:{port} failed: {err}"),
        );
        log_stream_access(&config, server, &log);
        return;
    }
    match tokio::time::timeout(
        server.proxy_connect_timeout,
        upstream.connect(upstream_address),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            log.status = 502;
            if let Some(guard) = guard.as_mut() {
                guard.mark_failed();
            }
            log.logger.log(
                LogLevel::Error,
                format!("stream UDP connect() to {host}:{port} failed: {err}"),
            );
            log_stream_access(&config, server, &log);
            return;
        }
        Err(_) => {
            log.status = 504;
            if let Some(guard) = guard.as_mut() {
                guard.mark_failed();
            }
            log.logger.log(
                LogLevel::Error,
                format!("stream UDP connect() to {host}:{port} timed out"),
            );
            log_stream_access(&config, server, &log);
            return;
        }
    }
    let connect = attempt_started.elapsed();
    let proxy_protocol_source = ListenAddress::Inet(remote);
    let proxy_protocol_destination = listener
        .local_addr()
        .map(ListenAddress::Inet)
        .unwrap_or_else(|_| route.address.clone());
    if !send_stream_proxy_protocol_datagram(
        &upstream,
        server,
        &proxy_protocol_source,
        &proxy_protocol_destination,
        &mut log,
    )
    .await
    {
        if let Some(guard) = guard.as_mut() {
            guard.mark_failed();
        }
        log_stream_access(&config, server, &log);
        return;
    }
    log.status = 200;
    log.upstream_connect_time = Some(connect);
    let settings = StreamProxySettings::from_server(server, &log);
    let mut upload_ready_at = tokio::time::Instant::now();
    let mut download_ready_at = tokio::time::Instant::now();
    let mut counts = UdpProxyCounts::new(server.proxy_requests, server.proxy_responses);
    let mut accepting_requests = true;
    let mut response = vec![0_u8; settings.buffer_size];
    let mut datagram_started = None;
    let idle = tokio::time::sleep(server.proxy_timeout);
    tokio::pin!(idle);
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || shutdown.borrow().requested() {
                    log_stream_access(&config, server, &log);
                    return;
                }
            }
            _ = tokio::time::sleep_until(upload_ready_at),
                if accepting_requests && upload_ready_at > tokio::time::Instant::now() => {}
            datagram = datagrams.recv(),
                if accepting_requests && upload_ready_at <= tokio::time::Instant::now() => {
                let Some(datagram) = datagram else {
                    log_stream_access(&config, server, &log);
                    return;
                };
                let datagram = &datagram[..datagram.len().min(settings.buffer_size)];
                log.bytes_received += datagram.len() as u64;
                log.upstream_bytes_sent += datagram.len() as u64;
                datagram_started = Some(Instant::now());
                if let Err(err) = upstream.send(datagram).await {
                    log.status = 502;
                    if let Some(guard) = guard.as_mut() {
                        guard.mark_failed();
                    }
                    log.logger.log(
                        LogLevel::Error,
                        format!("stream UDP send() to {host}:{port} failed: {err}"),
                    );
                    log_stream_access(&config, server, &log);
                    return;
                }
                if let Some(delay) = stream_rate_delay(datagram.len(), settings.upload_rate) {
                    upload_ready_at = tokio::time::Instant::now() + delay;
                }
                counts.record_request();
                accepting_requests = counts.accepts_requests();
                if !accepting_requests {
                    datagrams.close();
                }
                if counts.is_complete() {
                    log_stream_access(&config, server, &log);
                    return;
                }
                idle
                    .as_mut()
                    .reset(tokio::time::Instant::now() + server.proxy_timeout);
            }
            _ = tokio::time::sleep_until(download_ready_at),
                if download_ready_at > tokio::time::Instant::now() => {}
            received = upstream.recv(&mut response),
                if download_ready_at <= tokio::time::Instant::now() => {
                let size = match received {
                    Ok(size) => size,
                    Err(err) => {
                        log.status = 502;
                        if let Some(guard) = guard.as_mut() {
                            guard.mark_failed();
                        }
                        log.logger.log(
                            LogLevel::Info,
                            format!("stream UDP receive() from {host}:{port} failed: {err}"),
                        );
                        log_stream_access(&config, server, &log);
                        return;
                    }
                };
                if let Some(started) = datagram_started {
                    let elapsed = started.elapsed();
                    log.upstream_first_byte_time =
                        log.upstream_first_byte_time.or(Some(connect + elapsed));
                    log.upstream_session_time = Some(connect + elapsed);
                    if let Some(guard) = guard.as_mut() {
                        guard.record_timing(UpstreamAttemptTiming {
                            connect,
                            first_byte: Some(elapsed),
                            header: elapsed,
                            response: elapsed,
                        });
                    }
                    datagram_started = None;
                }
                counts.record_response();
                if let Err(err) = listener.send_to(&response[..size], remote).await {
                    log.logger.log(
                        LogLevel::Info,
                        format!("stream UDP send_to() {remote} failed: {err}"),
                    );
                } else {
                    log.bytes_sent += size as u64;
                    log.upstream_bytes_received += size as u64;
                }
                if let Some(delay) = stream_rate_delay(size, settings.download_rate) {
                    download_ready_at = tokio::time::Instant::now() + delay;
                }
                if counts.is_complete() {
                    log_stream_access(&config, server, &log);
                    return;
                }
                idle
                    .as_mut()
                    .reset(tokio::time::Instant::now() + server.proxy_timeout);
            }
            _ = &mut idle => {
                log_stream_access(&config, server, &log);
                return;
            }
        }
    }
}
