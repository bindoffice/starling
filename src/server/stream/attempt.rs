use super::*;

pub(in crate::server) struct StreamProxyContext<'a> {
    pub(in crate::server) remote: &'a ListenAddress,
    pub(in crate::server) server: &'a crate::config::StreamServer,
    pub(in crate::server) tls_default_name: &'a str,
    pub(in crate::server) attempt_started: Instant,
    pub(in crate::server) client_tls_cache: &'a ClientTlsConfigCache,
    pub(in crate::server) log: &'a mut StreamLogContext,
}

pub(in crate::server) enum StreamAttemptResult {
    Relayed {
        connect: Duration,
        relay: StreamRelayOutcome,
    },
    Failed {
        status: u16,
        connect: Duration,
        retryable: bool,
    },
}

pub(in crate::server) async fn proxy_stream_attempt<I>(
    inbound: &mut I,
    target: &ResolvedUpstreamTarget,
    context: StreamProxyContext<'_>,
) -> StreamAttemptResult
where
    I: AsyncRead + AsyncWrite + Unpin,
{
    let host = target.host.as_str();
    let port = target.port;
    let upstream = upstream_target_display(target);
    let local_bind = evaluate_stream_proxy_bind(context.server, context.log);
    let socket_options = StreamSocketOptions::from_server(context.server);
    if let Some(path) = host.strip_prefix("unix:") {
        #[cfg(unix)]
        {
            let mut outbound = match tokio::time::timeout(
                context.server.proxy_connect_timeout,
                tokio::net::UnixStream::connect(path),
            )
            .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(err)) => {
                    context.log.logger.log(
                        LogLevel::Error,
                        format!("connect() to {upstream} failed: {err}"),
                    );
                    return StreamAttemptResult::Failed {
                        status: 502,
                        connect: context.attempt_started.elapsed(),
                        retryable: true,
                    };
                }
                Err(_) => {
                    context.log.logger.log(
                        LogLevel::Error,
                        format!("connect() to {upstream} timed out"),
                    );
                    return StreamAttemptResult::Failed {
                        status: 504,
                        connect: context.attempt_started.elapsed(),
                        retryable: true,
                    };
                }
            };
            if let Err(err) = apply_stream_socket_options(&outbound, socket_options) {
                context.log.logger.log(
                    LogLevel::Error,
                    format!("setsockopt() for {upstream} failed: {err}"),
                );
                return StreamAttemptResult::Failed {
                    status: 502,
                    connect: context.attempt_started.elapsed(),
                    retryable: true,
                };
            }
            let proxy_protocol_destination = context.log.local.clone();
            if !send_stream_proxy_protocol(
                &mut outbound,
                context.server,
                context.remote,
                &proxy_protocol_destination,
                StreamProxyProtocolTransport::Stream,
                context.log,
            )
            .await
            {
                return StreamAttemptResult::Failed {
                    status: 502,
                    connect: context.attempt_started.elapsed(),
                    retryable: true,
                };
            }
            let connect = context.attempt_started.elapsed();
            proxy_stream_connection(inbound, outbound, context, connect).await
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            context.log.logger.log(
                LogLevel::Error,
                format!("connect() to {upstream} is unsupported on this platform"),
            );
            StreamAttemptResult::Failed {
                status: 502,
                connect: context.attempt_started.elapsed(),
                retryable: false,
            }
        }
    } else {
        let mut outbound = match tokio::time::timeout(
            context.server.proxy_connect_timeout,
            connect_stream_tcp(host, port, local_bind.as_deref(), socket_options),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(err)) => {
                context.log.logger.log(
                    LogLevel::Error,
                    format!("connect() to {host}:{port} failed: {err}"),
                );
                return StreamAttemptResult::Failed {
                    status: 502,
                    connect: context.attempt_started.elapsed(),
                    retryable: true,
                };
            }
            Err(_) => {
                context.log.logger.log(
                    LogLevel::Error,
                    format!("connect() to {host}:{port} timed out"),
                );
                return StreamAttemptResult::Failed {
                    status: 504,
                    connect: context.attempt_started.elapsed(),
                    retryable: true,
                };
            }
        };
        let proxy_protocol_destination = context.log.local.clone();
        if !send_stream_proxy_protocol(
            &mut outbound,
            context.server,
            context.remote,
            &proxy_protocol_destination,
            StreamProxyProtocolTransport::Stream,
            context.log,
        )
        .await
        {
            return StreamAttemptResult::Failed {
                status: 502,
                connect: context.attempt_started.elapsed(),
                retryable: true,
            };
        }
        let connect = context.attempt_started.elapsed();
        proxy_stream_connection(inbound, outbound, context, connect).await
    }
}

async fn proxy_stream_connection<I, O>(
    inbound: &mut I,
    outbound: O,
    context: StreamProxyContext<'_>,
    connect: Duration,
) -> StreamAttemptResult
where
    I: AsyncRead + AsyncWrite + Unpin,
    O: AsyncRead + AsyncWrite + Unpin,
{
    let StreamProxyContext {
        remote,
        server,
        tls_default_name,
        attempt_started,
        client_tls_cache,
        log,
    } = context;
    let settings = StreamProxySettings::from_server(server, log);
    let alpn_protocols = server
        .proxy_ssl_alpn
        .iter()
        .map(|template| eval_stream_log_variables(template, log))
        .filter(|protocol| !protocol.is_empty() && protocol.len() <= 255)
        .map(String::into_bytes)
        .collect::<Vec<_>>();
    if server.proxy_ssl {
        let tls_name = server
            .proxy_ssl_name
            .as_deref()
            .map(|template| eval_stream_log_variables(template, log))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| tls_default_name.to_string());
        let mut outbound = match tokio::time::timeout(
            server.proxy_connect_timeout,
            upgrade_tls_client(
                outbound,
                &tls_name,
                ClientTlsOptions {
                    verify: server.proxy_ssl_verify,
                    server_name: server.proxy_ssl_server_name,
                    session_reuse: server.proxy_ssl_session_reuse,
                    trusted_certificate: server.proxy_ssl_trusted_certificate.as_deref(),
                    verify_depth: server.proxy_ssl_verify_depth,
                    crl: server.proxy_ssl_crl.as_deref(),
                    client_certificate: server.proxy_ssl_certificate.as_deref(),
                    client_certificate_key: server.proxy_ssl_certificate_key.as_deref(),
                    client_certificate_passwords: &server.proxy_ssl_passwords,
                    protocols: server.proxy_ssl_protocols,
                    ciphers: &server.proxy_ssl_ciphers,
                    alpn_protocols: &alpn_protocols,
                },
                client_tls_cache,
            ),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(err)) => {
                log.logger.log(
                    LogLevel::Info,
                    format!("stream upstream TLS for {remote} failed: {err}"),
                );
                return StreamAttemptResult::Failed {
                    status: 502,
                    connect: attempt_started.elapsed(),
                    retryable: true,
                };
            }
            Err(_) => {
                log.logger.log(
                    LogLevel::Info,
                    format!("stream upstream TLS for {remote} timed out"),
                );
                return StreamAttemptResult::Failed {
                    status: 504,
                    connect: attempt_started.elapsed(),
                    retryable: true,
                };
            }
        };
        let relay = relay_stream(
            inbound,
            &mut outbound,
            server.proxy_timeout,
            settings,
            remote,
            &log.logger,
        )
        .await;
        return StreamAttemptResult::Relayed { connect, relay };
    }
    let mut outbound = outbound;
    let relay = relay_stream(
        inbound,
        &mut outbound,
        server.proxy_timeout,
        settings,
        remote,
        &log.logger,
    )
    .await;
    StreamAttemptResult::Relayed { connect, relay }
}
