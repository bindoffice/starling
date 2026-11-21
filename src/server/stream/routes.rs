use super::*;

pub(in crate::server) fn build_stream_routes(config: &RuntimeConfig) -> Vec<StreamRoute> {
    let Some(stream) = &config.stream else {
        return Vec::new();
    };
    let mut routes: Vec<StreamRoute> = Vec::new();
    for server in &stream.servers {
        for listen in &server.listens {
            if let Some(route) = routes
                .iter_mut()
                .find(|route| route.address == listen.address && route.udp == listen.udp)
            {
                route.ssl |= listen.ssl;
                route.proxy_protocol |= listen.proxy_protocol;
                if listen.socket_options_set {
                    route.socket_options = listen.socket_options.clone();
                }
                continue;
            }
            routes.push(StreamRoute {
                address: listen.address.clone(),
                ssl: listen.ssl,
                udp: listen.udp,
                proxy_protocol: listen.proxy_protocol,
                socket_options: listen.socket_options.clone(),
            });
        }
    }
    routes.sort_by_key(|route| route.address.clone());
    routes
}

pub(in crate::server) async fn stream_accept_loop(
    listener: TcpListener,
    route: StreamRoute,
    state: SharedState,
    mut shutdown: watch::Receiver<ShutdownMode>,
) {
    let mut connections = JoinSet::new();
    let shutdown_mode = loop {
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
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(err)) = joined {
                    stream_route_logger(&state, &route)
                        .await
                        .log(LogLevel::Error, format!("stream connection task failed: {err}"));
                }
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, transport_remote)) => {
                        let nodelay = stream_route_tcp_nodelay(&state, &route).await;
                        let _ = stream.set_nodelay(nodelay);
                        let local = stream
                            .local_addr()
                            .map(ListenAddress::Inet)
                            .unwrap_or_else(|_| route.address.clone());
                        let state = state.clone();
                        let route = route.clone();
                        connections.spawn(async move {
                            handle_stream_accepted(
                                stream,
                                ListenAddress::Inet(transport_remote),
                                local,
                                route,
                                state,
                            )
                            .await;
                        });
                    }
                    Err(err) => {
                        stream_route_logger(&state, &route)
                            .await
                            .log(LogLevel::Error, format!("stream accept() failed: {err}"));
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }
    };
    finish_connection_tasks(connections, shutdown_mode, shutdown).await;
}

#[cfg(unix)]
pub(in crate::server) async fn stream_unix_accept_loop(
    listener: UnixListener,
    route: StreamRoute,
    state: SharedState,
    mut shutdown: watch::Receiver<ShutdownMode>,
) {
    let mut connections = JoinSet::new();
    let shutdown_mode = loop {
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
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(err)) = joined {
                    stream_route_logger(&state, &route)
                        .await
                        .log(LogLevel::Error, format!("stream connection task failed: {err}"));
                }
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let transport_remote = unix_peer_address(&stream);
                        let local = route.address.clone();
                        let state = state.clone();
                        let route = route.clone();
                        connections.spawn(async move {
                            handle_stream_accepted(
                                stream,
                                transport_remote,
                                local,
                                route,
                                state,
                            )
                            .await;
                        });
                    }
                    Err(err) => {
                        stream_route_logger(&state, &route)
                            .await
                            .log(LogLevel::Error, format!("stream accept() failed: {err}"));
                        if err.kind() == io::ErrorKind::InvalidInput {
                            break ShutdownMode::Fast;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }
    };
    finish_connection_tasks(connections, shutdown_mode, shutdown).await;
}

pub(in crate::server) async fn handle_stream_accepted<S>(
    inbound: S,
    transport_remote: ListenAddress,
    local: ListenAddress,
    route: StreamRoute,
    state: SharedState,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let mut passes = 0;
    handle_stream_accepted_with_passes(inbound, transport_remote, local, route, state, &mut passes)
        .await;
}

pub(in crate::server) async fn handle_stream_accepted_with_passes<S>(
    mut inbound: S,
    transport_remote: ListenAddress,
    local: ListenAddress,
    route: StreamRoute,
    state: SharedState,
    passes: &mut usize,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let addresses = if route.proxy_protocol {
        let proxy_protocol_timeout = stream_proxy_protocol_timeout(&route, &state).await;
        match tokio::time::timeout(
            proxy_protocol_timeout,
            read_proxy_protocol(&mut inbound, &transport_remote),
        )
        .await
        {
            Ok(Ok(addresses)) => addresses,
            Ok(Err(err)) => {
                stream_route_logger(&state, &route).await.log(
                    LogLevel::Info,
                    format!("stream PROXY protocol header from {transport_remote} failed: {err}"),
                );
                return;
            }
            Err(_) => {
                stream_route_logger(&state, &route).await.log(
                    LogLevel::Info,
                    format!("stream PROXY protocol header from {transport_remote} timed out"),
                );
                return;
            }
        }
    } else {
        ProxyProtocolAddresses::without_header(&transport_remote)
    };
    handle_stream_connection_boxed(Box::new(inbound), &addresses, local, route, state, passes)
        .await;
}

async fn stream_proxy_protocol_timeout(route: &StreamRoute, state: &SharedState) -> Duration {
    state
        .config
        .read()
        .await
        .stream
        .as_ref()
        .and_then(|stream| stream_server_for_route(stream, route))
        .map(|server| server.proxy_protocol_timeout)
        .unwrap_or(Duration::from_secs(30))
}

async fn stream_route_tcp_nodelay(state: &SharedState, route: &StreamRoute) -> bool {
    state
        .config
        .read()
        .await
        .stream
        .as_ref()
        .and_then(|stream| stream_server_for_route(stream, route))
        .map(|server| server.tcp_nodelay)
        .unwrap_or(true)
}
