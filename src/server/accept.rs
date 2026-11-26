use super::*;

pub(in crate::server) async fn accept_loop(
    listener: TcpListener,
    group: ListenerGroup,
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
                log_connection_task_result(&state, joined);
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, transport_remote)) => {
                        let (stream, control) = match ClientSocketControl::attach_tcp(stream) {
                            Ok(stream) => stream,
                            Err(err) => {
                                state.logger.log(
                                    LogLevel::Error,
                                    format!("accept() socket setup failed: {err}"),
                                );
                                continue;
                            }
                        };
                        let state = state.clone();
                        let group = group.clone();
                        let shutdown = shutdown.clone();
                        connections.spawn(async move {
                            let local = stream
                                .local_addr()
                                .map(ListenAddress::Inet)
                                .unwrap_or_else(|_| group.address.clone());
                            let connection = ConnectionAddresses::direct(
                                ListenAddress::Inet(transport_remote),
                                local,
                            );
                            let reset_target = control
                                .as_ref()
                                .map(|control| TimeoutReset::from_control(control.clone()));
                            handle_http_connection_with_shutdown(
                                stream,
                                connection,
                                group,
                                state,
                                reset_target,
                                control,
                                Some(shutdown),
                            )
                            .await;
                        });
                        // Keep accept from starving already accepted connections when
                        // the listener remains continuously readable.
                        tokio::task::yield_now().await;
                    }
                    Err(err) => {
                        state.logger.log(LogLevel::Error, format!("accept() failed: {err}"));
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

pub(in crate::server) fn log_connection_task_result(
    state: &SharedState,
    joined: Option<Result<(), tokio::task::JoinError>>,
) {
    if let Some(Err(err)) = joined {
        state
            .logger
            .log(LogLevel::Error, format!("connection task failed: {err}"));
    }
}

#[cfg(unix)]
pub(in crate::server) async fn accept_unix_loop(
    listener: UnixListener,
    group: ListenerGroup,
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
                log_connection_task_result(&state, joined);
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let (stream, control) = match ClientSocketControl::attach_unix(stream) {
                            Ok(stream) => stream,
                            Err(err) => {
                                state.logger.log(
                                    LogLevel::Error,
                                    format!("accept() socket setup failed: {err}"),
                                );
                                continue;
                            }
                        };
                        let remote = unix_peer_address(&stream);
                        let local = group.address.clone();
                        let state = state.clone();
                        let group = group.clone();
                        let shutdown = shutdown.clone();
                        connections.spawn(async move {
                            let connection = ConnectionAddresses::direct(remote, local);
                            let reset_target = control
                                .as_ref()
                                .map(|control| TimeoutReset::from_control(control.clone()));
                            handle_http_connection_with_shutdown(
                                stream,
                                connection,
                                group,
                                state,
                                reset_target,
                                control,
                                Some(shutdown),
                            )
                            .await;
                        });
                        tokio::task::yield_now().await;
                    }
                    Err(err) => {
                        state.logger.log(LogLevel::Error, format!("accept() failed: {err}"));
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

#[cfg(unix)]
pub(in crate::server) fn unix_peer_address(stream: &UnixStream) -> ListenAddress {
    stream
        .peer_addr()
        .ok()
        .and_then(|address| address.as_pathname().map(Path::to_path_buf))
        .map(ListenAddress::Unix)
        .unwrap_or_else(|| ListenAddress::Unix(PathBuf::new()))
}

#[cfg(test)]
pub(in crate::server) async fn handle_http_connection<S>(
    stream: S,
    connection: ConnectionAddresses,
    group: ListenerGroup,
    state: SharedState,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    handle_http_connection_with_reset(stream, connection, group, state, None, None).await;
}

#[cfg(test)]
pub(in crate::server) async fn handle_http_connection_with_reset<S>(
    stream: S,
    connection: ConnectionAddresses,
    group: ListenerGroup,
    state: SharedState,
    reset_target: Option<TimeoutReset>,
    control: Option<ClientSocketControl>,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    handle_http_connection_with_shutdown(
        stream,
        connection,
        group,
        state,
        reset_target,
        control,
        None,
    )
    .await;
}

pub(in crate::server) async fn handle_http_connection_with_shutdown<S>(
    mut stream: S,
    mut connection: ConnectionAddresses,
    group: ListenerGroup,
    state: SharedState,
    reset_target: Option<TimeoutReset>,
    control: Option<ClientSocketControl>,
    mut shutdown: Option<watch::Receiver<ShutdownMode>>,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    if group.proxy_protocol {
        let transport_remote = connection.remote.clone();
        let header = tokio::time::timeout(
            Duration::from_secs(5),
            read_proxy_protocol(&mut stream, &transport_remote),
        );
        let shutdown_enabled = shutdown.is_some();
        let header = tokio::select! {
            header = header => header,
            _ = wait_for_optional_shutdown(&mut shutdown), if shutdown_enabled => return,
        };
        match header {
            Ok(Ok(addresses)) => {
                connection.proxy_protocol_addr = addresses.proxy_protocol_addr;
                connection.proxy_protocol_port = addresses.proxy_protocol_port;
                connection.proxy_protocol_server_addr = addresses.proxy_protocol_server_addr;
                connection.proxy_protocol_server_port = addresses.proxy_protocol_server_port;
                connection.proxy_protocol_tlvs = addresses.proxy_protocol_tlvs;
            }
            Ok(Err(err)) => {
                state.logger.log(
                    LogLevel::Info,
                    format!("PROXY protocol header from {transport_remote} failed: {err}"),
                );
                return;
            }
            Err(_) => {
                state.logger.log(
                    LogLevel::Info,
                    format!("PROXY protocol header from {transport_remote} timed out"),
                );
                return;
            }
        }
    }
    if group.ssl {
        if let Some(control) = &control {
            let enabled = {
                let config = state.current_config();
                connection_server_context(&config, &group)
                    .map(|server| server.context.tcp_nodelay)
                    .unwrap_or(true)
            };
            if let Err(err) = control.apply_nodelay_once(enabled) {
                state.logger.log(
                    LogLevel::Alert,
                    format!("setsockopt(TCP_NODELAY) failed: {err}"),
                );
                return;
            }
        }
        let tls_configs = { state.tls_configs.read().await.get(&group.address).cloned() };
        let Some(tls_configs) = tls_configs else {
            state.logger.log(
                LogLevel::Error,
                format!("no TLS configuration for listener {}", group.address),
            );
            return;
        };
        let handshake = tokio::time::timeout(
            Duration::from_secs(15),
            accept_http_tls(stream, tls_configs),
        );
        let shutdown_enabled = shutdown.is_some();
        let handshake = tokio::select! {
            handshake = handshake => handshake,
            _ = wait_for_optional_shutdown(&mut shutdown), if shutdown_enabled => return,
        };
        match handshake {
            Ok(Ok(stream)) => {
                handle_tls_stream_with_reset(
                    stream,
                    connection,
                    group,
                    state,
                    reset_target,
                    control,
                    shutdown,
                )
                .await;
            }
            Ok(Err(err)) => {
                let message = format!("SSL_do_handshake() failed: {err}");
                if let Some((mut stream, ocsp_error, session_reused)) =
                    err.into_client_ocsp_failure()
                {
                    state.logger.log(LogLevel::Info, message);
                    if let Err(error) =
                        write_client_ocsp_rejection(&mut *stream, &ocsp_error, session_reused).await
                    {
                        state.logger.log(
                            LogLevel::Info,
                            format!("failed to send OCSP rejection response: {error}"),
                        );
                    }
                } else {
                    state.logger.log(LogLevel::Info, message);
                }
            }
            Err(_) => {
                state.logger.log(LogLevel::Info, "SSL handshake timed out");
            }
        }
    } else {
        if group.http2 {
            handle_plaintext_http_connection(
                stream,
                connection,
                group,
                state,
                reset_target,
                control,
                shutdown,
            )
            .await;
        } else {
            handle_io_with_shutdown(
                stream,
                connection,
                group,
                state,
                reset_target,
                control,
                shutdown,
            )
            .await;
        }
    }
}

async fn write_client_ocsp_rejection<S>(
    stream: &mut tokio_rustls::server::TlsStream<S>,
    error: &io::Error,
    session_reused: bool,
) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let reason = client_certificate_verify_failure(error);
    let reused = if session_reused { ":r" } else { "" };
    let body = format!(
        "<html>\r\n<head><title>400 Bad Request</title></head>\r\n<body>\r\n<center><h1>400 Bad Request</h1></center>\r\n<center>client certificate verify error: FAILED:{reason}{reused}</center>\r\n</body>\r\n</html>\r\n"
    );
    let head = format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.flush().await?;
    stream.shutdown().await
}

#[cfg(test)]
pub(in crate::server) async fn handle_tls_stream<S>(
    stream: tokio_rustls::server::TlsStream<S>,
    connection: ConnectionAddresses,
    group: ListenerGroup,
    state: SharedState,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    handle_tls_stream_with_reset(stream, connection, group, state, None, None, None).await;
}

async fn handle_tls_stream_with_reset<S>(
    stream: tokio_rustls::server::TlsStream<S>,
    mut connection: ConnectionAddresses,
    group: ListenerGroup,
    state: SharedState,
    reset_target: Option<TimeoutReset>,
    control: Option<ClientSocketControl>,
    shutdown: Option<watch::Receiver<ShutdownMode>>,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    connection.tls = Some(tls_connection_info(stream.get_ref().1));
    let negotiated_http2 =
        group.http2 && stream.get_ref().1.alpn_protocol() == Some(b"h2".as_slice());
    if negotiated_http2 {
        handle_h2(stream, connection, group, state, control, shutdown).await;
    } else {
        handle_io_with_shutdown(
            stream,
            connection,
            group,
            state,
            reset_target,
            control,
            shutdown,
        )
        .await;
    }
}
