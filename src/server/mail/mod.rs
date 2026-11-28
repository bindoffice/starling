use super::*;
mod auth_http;
mod protocol;
use auth_http::*;
use protocol::*;

use crate::server::tls::{MailClientCertificateStatus, MailClientCertificateVerifier};

pub(super) trait MailIo: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> MailIo for T where T: AsyncRead + AsyncWrite + Send + Unpin {}

pub(super) type BoxedMailStream = Box<dyn MailIo>;
pub(super) type MailReader = BufReader<BoxedMailStream>;

#[derive(Debug, Clone)]
pub(in crate::server) struct MailRoute {
    pub(in crate::server) address: ListenAddress,
    pub(in crate::server) ssl: bool,
    pub(in crate::server) proxy_protocol: bool,
    pub(in crate::server) socket_options: ListenSocketOptions,
    pub(in crate::server) default_server_id: usize,
}

#[derive(Clone)]
pub(in crate::server) struct MailTlsRuntimeConfig {
    server: Arc<tokio_rustls::rustls::ServerConfig>,
    client_certificate: Option<Arc<MailClientCertificateVerifier>>,
}

pub(in crate::server) fn build_mail_routes(config: &RuntimeConfig) -> Vec<MailRoute> {
    let Some(mail) = &config.mail else {
        return Vec::new();
    };
    let mut routes: Vec<MailRoute> = Vec::new();
    for server in &mail.servers {
        for listen in &server.listens {
            if let Some(route) = routes
                .iter_mut()
                .find(|route| route.address == listen.address)
            {
                route.ssl |= listen.ssl;
                route.proxy_protocol |= listen.proxy_protocol;
                if listen.default_server {
                    route.default_server_id = server.id;
                }
                if listen.socket_options_set {
                    route.socket_options = listen.socket_options.clone();
                }
                continue;
            }
            routes.push(MailRoute {
                address: listen.address.clone(),
                ssl: listen.ssl,
                proxy_protocol: listen.proxy_protocol,
                socket_options: listen.socket_options.clone(),
                default_server_id: server.id,
            });
        }
    }
    routes.sort_by_key(|route| route.address.clone());
    routes
}

pub(in crate::server) fn mail_server_for_route<'a>(
    mail: &'a MailConfig,
    route: &MailRoute,
) -> Option<&'a MailServer> {
    mail.servers
        .iter()
        .find(|server| {
            server.id == route.default_server_id
                && server
                    .listens
                    .iter()
                    .any(|listen| listen.address == route.address)
        })
        .or_else(|| {
            mail.servers.iter().find(|server| {
                server
                    .listens
                    .iter()
                    .any(|listen| listen.address == route.address)
            })
        })
}

pub(in crate::server) fn build_mail_loggers(
    config: &RuntimeConfig,
) -> io::Result<HashMap<usize, Arc<Logger>>> {
    let Some(mail) = &config.mail else {
        return Ok(HashMap::new());
    };
    let mut loggers = HashMap::new();
    for server in &mail.servers {
        if server.error_logs.is_empty() {
            continue;
        }
        loggers.insert(
            server.id,
            Arc::new(Logger::new(&server.error_logs, &config.prefix)?),
        );
    }
    Ok(loggers)
}

pub(in crate::server) fn build_mail_resolvers(
    config: &RuntimeConfig,
) -> HashMap<usize, crate::server::resolver::Resolver> {
    let Some(mail) = &config.mail else {
        return HashMap::new();
    };
    mail.servers
        .iter()
        .map(|server| {
            (
                server.id,
                crate::server::resolver::Resolver::new(
                    server.resolver.clone(),
                    server.resolver_timeout,
                ),
            )
        })
        .collect()
}

pub(in crate::server) async fn mail_logger(
    state: &SharedState,
    server: &MailServer,
) -> Arc<Logger> {
    state
        .mail_loggers
        .read()
        .await
        .get(&server.id)
        .cloned()
        .unwrap_or_else(|| state.logger.clone())
}

pub(in crate::server) async fn reopen_mail_loggers(state: &SharedState) {
    for logger in state.mail_loggers.read().await.values() {
        logger.reopen();
    }
}

pub(in crate::server) fn build_mail_tls_configs(
    config: &RuntimeConfig,
) -> io::Result<HashMap<ListenAddress, Arc<MailTlsRuntimeConfig>>> {
    let Some(mail) = &config.mail else {
        return Ok(HashMap::new());
    };
    let mut configs = HashMap::new();
    for route in build_mail_routes(config) {
        let Some(server) = mail_server_for_route(mail, &route) else {
            continue;
        };
        if !route.ssl && server.tls.starttls == MailStartTls::Off {
            continue;
        }
        configs.insert(route.address.clone(), build_mail_tls_config(server)?);
    }
    Ok(configs)
}

fn build_mail_tls_config(server: &MailServer) -> io::Result<Arc<MailTlsRuntimeConfig>> {
    install_crypto_provider();
    let versions = rustls_protocol_versions(server.tls.protocols);
    let provider = Arc::new(tls_crypto_provider(&server.tls.ciphers));
    let builder = tokio_rustls::rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&versions)
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid mail TLS cipher configuration: {error}"),
            )
        })?;
    let certificates = load_certificate_entries(
        &server.tls.certificates,
        &server.tls.certificate_keys,
        server.tls.certificate.as_deref(),
        server.tls.certificate_key.as_deref(),
        &server.tls.passwords,
    )?;
    let entries = certificates
        .into_iter()
        .map(|certificate| ServerCertificateEntry::new(certificate, None))
        .collect();
    let resolver = Arc::new(ServerCertificateResolver::new(entries));
    let client_verifier = build_mail_client_cert_verifier(
        server.tls.verify_client,
        server.tls.client_certificate.as_deref(),
        &provider,
    )?;
    let mut config = match client_verifier {
        Some(verifier) => builder
            .with_client_cert_verifier(verifier)
            .with_cert_resolver(resolver),
        None => builder.with_no_client_auth().with_cert_resolver(resolver),
    };
    config.ignore_client_order = server.tls.prefer_server_ciphers;
    config.alpn_protocols = vec![server.protocol.alpn().to_vec()];
    configure_mail_session_cache(server, &mut config)?;
    let client_certificate = build_mail_client_certificate_verifier(
        server.tls.verify_client,
        server.tls.verify_depth,
        server.tls.client_certificate.as_deref(),
        server.tls.trusted_certificate.as_deref(),
        server.tls.crl.as_deref(),
    )?;
    Ok(Arc::new(MailTlsRuntimeConfig {
        server: Arc::new(config),
        client_certificate,
    }))
}

fn configure_mail_session_cache(
    server: &MailServer,
    config: &mut tokio_rustls::rustls::ServerConfig,
) -> io::Result<()> {
    use tokio_rustls::rustls::crypto::ring::Ticketer;
    use tokio_rustls::rustls::server::{NoServerSessionStorage, ServerSessionMemoryCache};

    let (storage, stateful_cache): (
        Arc<dyn tokio_rustls::rustls::server::StoresServerSessions>,
        bool,
    ) = match &server.tls.session_cache {
        MailSessionCache::Off | MailSessionCache::None => {
            (Arc::new(NoServerSessionStorage {}), false)
        }
        MailSessionCache::Builtin(Some(size)) => (ServerSessionMemoryCache::new(*size), true),
        MailSessionCache::Builtin(None) => (ServerSessionMemoryCache::new(20_480), true),
        MailSessionCache::Shared { name: _, size } => (
            ServerSessionMemoryCache::new((*size / 2_048).max(128)),
            true,
        ),
    };
    config.session_storage = storage;
    if server.tls.session_tickets {
        config.ticketer = Ticketer::new().map_err(|error| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("cannot initialize mail TLS session ticket keys: {error}"),
            )
        })?;
    }
    config.send_tls13_tickets = if server.tls.session_tickets || stateful_cache {
        2
    } else {
        0
    };
    Ok(())
}

pub(in crate::server) async fn mail_accept_loop(
    listener: TcpListener,
    route: MailRoute,
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
                if let Some(Err(error)) = joined {
                    state.logger.log(
                        LogLevel::Error,
                        format!("mail connection task failed: {error}"),
                    );
                }
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, remote)) => {
                        let _ = stream.set_nodelay(true);
                        connections.spawn(handle_mail_accepted(
                            stream,
                            ListenAddress::Inet(remote),
                            route.clone(),
                            state.clone(),
                        ));
                    }
                    Err(error) => {
                        state.logger.log(
                            LogLevel::Error,
                            format!("mail accept() failed: {error}"),
                        );
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }
    };
    finish_connection_tasks(connections, shutdown_mode, shutdown).await;
}

#[cfg(unix)]
pub(in crate::server) async fn mail_unix_accept_loop(
    listener: UnixListener,
    route: MailRoute,
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
                if let Some(Err(error)) = joined {
                    state.logger.log(
                        LogLevel::Error,
                        format!("mail UNIX connection task failed: {error}"),
                    );
                }
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let remote = unix_peer_address(&stream);
                        connections.spawn(handle_mail_accepted(
                            stream,
                            remote,
                            route.clone(),
                            state.clone(),
                        ));
                    }
                    Err(error) => {
                        state.logger.log(
                            LogLevel::Error,
                            format!("mail UNIX accept() failed: {error}"),
                        );
                        if error.kind() == io::ErrorKind::InvalidInput {
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

pub(in crate::server) async fn handle_mail_accepted<S>(
    mut inbound: S,
    transport_remote: ListenAddress,
    route: MailRoute,
    state: SharedState,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let server = {
        let config = state.current_config();
        let Some(mail) = config.mail.as_ref() else {
            return;
        };
        let Some(server) = mail_server_for_route(mail, &route) else {
            return;
        };
        server.clone()
    };
    let logger = mail_logger(&state, &server).await;
    logger.log(
        LogLevel::Info,
        format!("client {} connected to {}", transport_remote, route.address),
    );
    let addresses = if route.proxy_protocol {
        match tokio::time::timeout(
            server.timeout,
            read_proxy_protocol(&mut inbound, &transport_remote),
        )
        .await
        {
            Ok(Ok(addresses)) => addresses,
            Ok(Err(error)) => {
                logger.log(
                    LogLevel::Info,
                    format!("mail PROXY protocol header from {transport_remote} failed: {error}"),
                );
                return;
            }
            Err(_) => {
                logger.log(
                    LogLevel::Info,
                    format!("mail PROXY protocol header from {transport_remote} timed out"),
                );
                return;
            }
        }
    } else {
        ProxyProtocolAddresses::without_header(&transport_remote)
    };
    let mut context = SessionContext {
        remote: effective_mail_remote(&server, &addresses),
        local: route.address.clone(),
        proxy_addresses: addresses,
        client_host: None,
        tls: None,
        client_certificate_verify: None,
    };
    let mut inbound: BoxedMailStream = Box::new(inbound);
    if route.ssl {
        let Some(tls_config) = state
            .mail_tls_configs
            .read()
            .await
            .get(&route.address)
            .cloned()
        else {
            logger.log(
                LogLevel::Error,
                format!(
                    "missing TLS configuration for mail listener {}",
                    route.address
                ),
            );
            return;
        };
        match accept_mail_tls(inbound, &tls_config, server.timeout).await {
            Ok((stream, mut info, certificates)) => {
                let verification = verify_mail_client_certificate(
                    &server,
                    tls_config.client_certificate.as_deref(),
                    &certificates,
                    &mut info,
                );
                let verification = match verification {
                    Ok(verification) => verification,
                    Err(rejection) => {
                        let mut reader =
                            BufReader::with_capacity(server.client_buffer_size, stream);
                        let _ = write_client(
                            &mut reader,
                            mail_tls_certificate_error(server.protocol, rejection),
                            server.timeout,
                        )
                        .await;
                        return;
                    }
                };
                inbound = stream;
                context.tls = Some(info);
                context.client_certificate_verify = verification;
            }
            Err(error) => {
                logger.log(
                    LogLevel::Info,
                    format!("mail TLS handshake for {} failed: {error}", context.remote),
                );
                return;
            }
        }
    }
    if server.protocol == MailProtocol::Smtp {
        context.client_host =
            Some(mail_client_host(&state, &server, &context, logger.as_ref()).await);
    }

    loop {
        let mut reader = BufReader::with_capacity(server.client_buffer_size, inbound);
        match authenticate(&mut reader, &server, &context).await {
            Ok(AuthOutcome::Closed) => return,
            Ok(AuthOutcome::StartTls) => {
                if !reader.buffer().is_empty() {
                    logger.log(
                        LogLevel::Info,
                        "mail client pipelined data after STARTTLS command",
                    );
                    return;
                }
                let Some(tls_config) = state
                    .mail_tls_configs
                    .read()
                    .await
                    .get(&route.address)
                    .cloned()
                else {
                    let _ = write_client(
                        &mut reader,
                        mail_internal_error(server.protocol),
                        server.timeout,
                    )
                    .await;
                    return;
                };
                let stream = reader.into_inner();
                match accept_mail_tls(stream, &tls_config, server.timeout).await {
                    Ok((stream, mut info, certificates)) => {
                        let verification = verify_mail_client_certificate(
                            &server,
                            tls_config.client_certificate.as_deref(),
                            &certificates,
                            &mut info,
                        );
                        let verification = match verification {
                            Ok(verification) => verification,
                            Err(rejection) => {
                                let mut reader =
                                    BufReader::with_capacity(server.client_buffer_size, stream);
                                let _ = write_client(
                                    &mut reader,
                                    mail_tls_certificate_error(server.protocol, rejection),
                                    server.timeout,
                                )
                                .await;
                                return;
                            }
                        };
                        inbound = stream;
                        context.tls = Some(info);
                        context.client_certificate_verify = verification;
                    }
                    Err(error) => {
                        logger.log(
                            LogLevel::Info,
                            format!(
                                "mail STARTTLS handshake for {} failed: {error}",
                                context.remote
                            ),
                        );
                        return;
                    }
                }
            }
            Ok(AuthOutcome::Authenticated(auth)) => {
                proxy_mail(reader, &server, &context, &auth, &logger).await;
                return;
            }
            Err(error) => {
                logger.log(
                    LogLevel::Info,
                    format!("mail session for {} failed: {error}", context.remote),
                );
                if error.kind() != io::ErrorKind::ConnectionAborted {
                    let _ = write_client(
                        &mut reader,
                        mail_internal_error(server.protocol),
                        server.timeout,
                    )
                    .await;
                }
                return;
            }
        }
    }
}

async fn mail_client_host(
    state: &SharedState,
    server: &MailServer,
    context: &SessionContext,
    logger: &Logger,
) -> String {
    const UNAVAILABLE: &str = "[UNAVAILABLE]";
    const TEMPUNAVAIL: &str = "[TEMPUNAVAIL]";

    if !server.resolver.is_configured() {
        return UNAVAILABLE.into();
    }
    let Some(address) = context.remote.socket_addr().map(|address| address.ip()) else {
        return TEMPUNAVAIL.into();
    };
    let resolver = state.mail_resolvers.read().await.get(&server.id).cloned();
    let Some(resolver) = resolver else {
        return TEMPUNAVAIL.into();
    };
    match resolver.reverse_lookup(address).await {
        Ok(host) => host,
        Err(error) => {
            logger.log(
                LogLevel::Error,
                format!("{address} could not be resolved: {error}"),
            );
            if error.kind() == io::ErrorKind::NotFound {
                UNAVAILABLE.into()
            } else {
                TEMPUNAVAIL.into()
            }
        }
    }
}

async fn accept_mail_tls(
    inbound: BoxedMailStream,
    config: &MailTlsRuntimeConfig,
    timeout: Duration,
) -> io::Result<(
    BoxedMailStream,
    TlsConnectionInfo,
    Vec<CertificateDer<'static>>,
)> {
    let acceptor = tokio_rustls::TlsAcceptor::from(config.server.clone());
    match tokio::time::timeout(timeout, acceptor.accept(inbound)).await {
        Ok(Ok(stream)) => {
            let info = tls_connection_info(stream.get_ref().1);
            let peer_certificates = stream
                .get_ref()
                .1
                .peer_certificates()
                .map(|certificates| certificates.to_vec())
                .unwrap_or_default();
            Ok((Box::new(stream), info, peer_certificates))
        }
        Ok(Err(error)) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("TLS handshake failed: {error}"),
        )),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "TLS handshake timed out",
        )),
    }
}

#[derive(Debug, Clone, Copy)]
enum MailTlsCertificateRejection {
    Missing,
    Invalid,
}

fn verify_mail_client_certificate(
    server: &MailServer,
    verifier: Option<&MailClientCertificateVerifier>,
    certificates: &[CertificateDer<'_>],
    info: &mut TlsConnectionInfo,
) -> Result<Option<String>, MailTlsCertificateRejection> {
    let Some(verifier) = verifier else {
        return Ok(None);
    };
    match verifier.verify(certificates) {
        MailClientCertificateStatus::None => {
            if server.tls.verify_client == ClientCertificateVerify::On {
                Err(MailTlsCertificateRejection::Missing)
            } else {
                Ok(Some(MailClientCertificateStatus::None.as_auth_http_value()))
            }
        }
        MailClientCertificateStatus::Success => {
            info.client_certificate_verified = true;
            Ok(Some(
                MailClientCertificateStatus::Success.as_auth_http_value(),
            ))
        }
        MailClientCertificateStatus::Failed(error) => {
            info.client_certificate_error = Some(error.clone());
            if server.tls.verify_client == ClientCertificateVerify::OptionalNoCa {
                Ok(Some(format!("FAILED:{error}")))
            } else {
                Err(MailTlsCertificateRejection::Invalid)
            }
        }
    }
}

fn mail_tls_certificate_error(
    protocol: MailProtocol,
    rejection: MailTlsCertificateRejection,
) -> &'static [u8] {
    match (protocol, rejection) {
        (MailProtocol::Pop3, MailTlsCertificateRejection::Invalid) => {
            b"-ERR SSL certificate error\r\n"
        }
        (MailProtocol::Pop3, MailTlsCertificateRejection::Missing) => {
            b"-ERR No required SSL certificate\r\n"
        }
        (MailProtocol::Imap, MailTlsCertificateRejection::Invalid) => {
            b"* BYE SSL certificate error\r\n"
        }
        (MailProtocol::Imap, MailTlsCertificateRejection::Missing) => {
            b"* BYE No required SSL certificate\r\n"
        }
        (MailProtocol::Smtp, MailTlsCertificateRejection::Invalid) => {
            b"421 4.7.1 SSL certificate error\r\n"
        }
        (MailProtocol::Smtp, MailTlsCertificateRejection::Missing) => {
            b"421 4.7.1 No required SSL certificate\r\n"
        }
    }
}

async fn proxy_mail(
    mut client: MailReader,
    server: &MailServer,
    context: &SessionContext,
    auth: &AuthContext,
    logger: &Logger,
) {
    let Some((host, port)) = auth.backend.as_ref() else {
        let _ = write_client(
            &mut client,
            mail_internal_error(server.protocol),
            server.timeout,
        )
        .await;
        return;
    };
    let upstream = match tokio::time::timeout(
        server.timeout,
        TcpStream::connect((host.as_str(), *port)),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            logger.log(
                LogLevel::Error,
                format!("mail upstream {host}:{port} connect failed: {error}"),
            );
            let _ = write_client(
                &mut client,
                mail_internal_error(server.protocol),
                server.timeout,
            )
            .await;
            return;
        }
        Err(_) => {
            logger.log(
                LogLevel::Error,
                format!("mail upstream {host}:{port} connect timed out"),
            );
            let _ = write_client(
                &mut client,
                mail_internal_error(server.protocol),
                server.timeout,
            )
            .await;
            return;
        }
    };
    let _ = upstream.set_nodelay(true);
    let mut upstream: MailReader =
        BufReader::with_capacity(server.proxy.buffer_size, Box::new(upstream));
    if server.proxy.proxy_protocol != MailProxyProtocol::Off
        && let Err(error) = write_mail_proxy_protocol(
            upstream.get_mut(),
            server.proxy.proxy_protocol,
            &context.remote,
            &context.local,
            server.timeout,
        )
        .await
    {
        logger.log(
            LogLevel::Error,
            format!("mail upstream PROXY protocol write failed: {error}"),
        );
        let _ = write_client(
            &mut client,
            mail_internal_error(server.protocol),
            server.timeout,
        )
        .await;
        return;
    }

    let ready = match server.protocol {
        MailProtocol::Pop3 => {
            proxy_pop3_auth(&mut client, &mut upstream, server, context, auth, logger).await
        }
        MailProtocol::Imap => {
            proxy_imap_auth(&mut client, &mut upstream, server, auth, logger).await
        }
        MailProtocol::Smtp => {
            proxy_smtp_auth(&mut client, &mut upstream, server, context, auth, logger).await
        }
    };
    if !ready {
        return;
    }
    let mut inbound = client;
    let mut outbound = upstream;
    let settings = crate::server::stream::StreamProxySettings {
        upload_rate: 0,
        download_rate: 0,
        buffer_size: server.proxy.buffer_size,
        half_close: true,
    };
    let _ = crate::server::stream::relay_stream(
        &mut inbound,
        &mut outbound,
        server.proxy.timeout,
        settings,
        &context.remote,
        logger,
    )
    .await;
}

async fn proxy_pop3_auth(
    client: &mut MailReader,
    upstream: &mut MailReader,
    server: &MailServer,
    context: &SessionContext,
    auth: &AuthContext,
    logger: &Logger,
) -> bool {
    let Some(greeting) = read_upstream_line(upstream, server).await else {
        upstream_error(client, server, logger, "POP3 greeting").await;
        return false;
    };
    if !greeting.starts_with(b"+OK") {
        upstream_error(client, server, logger, "POP3 greeting").await;
        return false;
    }
    let mut command = b"USER ".to_vec();
    command.extend_from_slice(&auth.user);
    command.extend_from_slice(b"\r\n");
    if let Err(error) = write_client(upstream, &command, server.timeout).await {
        upstream_error(client, server, logger, &error.to_string()).await;
        return false;
    }
    let Some(response) = read_upstream_line(upstream, server).await else {
        upstream_error(client, server, logger, "POP3 USER response").await;
        return false;
    };
    if !response.starts_with(b"+OK") {
        let _ = write_client(client, &response, server.timeout).await;
        return false;
    }
    let mut command = b"PASS ".to_vec();
    command.extend_from_slice(&auth.password);
    command.extend_from_slice(b"\r\n");
    if let Err(error) = write_client(upstream, &command, server.timeout).await {
        upstream_error(client, server, logger, &error.to_string()).await;
        return false;
    }
    let Some(response) = read_upstream_line(upstream, server).await else {
        upstream_error(client, server, logger, "POP3 PASS response").await;
        return false;
    };
    let ok = response.starts_with(b"+OK");
    let _ = write_client(client, &response, server.timeout).await;
    if ok {
        logger.log(
            LogLevel::Info,
            format!("mail client {} logged in to {context:?}", context.remote),
        );
    }
    ok
}

async fn proxy_imap_auth(
    client: &mut MailReader,
    upstream: &mut MailReader,
    server: &MailServer,
    auth: &AuthContext,
    logger: &Logger,
) -> bool {
    let Some(greeting) = read_upstream_line(upstream, server).await else {
        upstream_error(client, server, logger, "IMAP greeting").await;
        return false;
    };
    if !greeting.starts_with(b"* OK") && !greeting.starts_with(b"* PREAUTH") {
        upstream_error(client, server, logger, "IMAP greeting").await;
        return false;
    }
    let tag = if auth.imap_tag.is_empty() {
        b"a".as_slice()
    } else {
        auth.imap_tag.as_slice()
    };
    let mut login = tag.to_vec();
    login.extend_from_slice(format!(" LOGIN {{{}}}\r\n", auth.user.len()).as_bytes());
    if write_client(upstream, &login, server.timeout)
        .await
        .is_err()
    {
        upstream_error(client, server, logger, "IMAP LOGIN").await;
        return false;
    }
    if !read_imap_continuation(upstream, server).await {
        return false;
    }
    let mut user = auth.user.clone();
    user.extend_from_slice(format!(" {{{}}}\r\n", auth.password.len()).as_bytes());
    if write_client(upstream, &user, server.timeout).await.is_err() {
        upstream_error(client, server, logger, "IMAP username").await;
        return false;
    }
    if !read_imap_continuation(upstream, server).await {
        return false;
    }
    let mut password = auth.password.clone();
    password.extend_from_slice(b"\r\n");
    if write_client(upstream, &password, server.timeout)
        .await
        .is_err()
    {
        upstream_error(client, server, logger, "IMAP password").await;
        return false;
    }
    for _ in 0..64 {
        let Some(response) = read_upstream_line(upstream, server).await else {
            upstream_error(client, server, logger, "IMAP LOGIN response").await;
            return false;
        };
        if write_client(client, &response, server.timeout)
            .await
            .is_err()
        {
            return false;
        }
        if response.starts_with(tag) {
            return response.get(tag.len() + 1..tag.len() + 3) == Some(b"OK");
        }
        if !response.starts_with(b"* ") {
            upstream_error(client, server, logger, "IMAP LOGIN response").await;
            return false;
        }
    }
    upstream_error(client, server, logger, "IMAP LOGIN response").await;
    false
}

async fn read_imap_continuation(upstream: &mut MailReader, server: &MailServer) -> bool {
    loop {
        let Some(line) = read_upstream_line(upstream, server).await else {
            return false;
        };
        if line.starts_with(b"+") {
            return true;
        }
        if line.starts_with(b"*") {
            continue;
        }
        return false;
    }
}

async fn proxy_smtp_auth(
    client: &mut MailReader,
    upstream: &mut MailReader,
    server: &MailServer,
    context: &SessionContext,
    auth: &AuthContext,
    logger: &Logger,
) -> bool {
    if read_smtp_response(upstream, server).await.is_none() {
        upstream_error(client, server, logger, "SMTP greeting").await;
        return false;
    }
    let esmtp = !auth.smtp_helo.is_empty();
    let command = if esmtp || server.proxy.xclient {
        format!("EHLO {}\r\n", server.server_name)
    } else {
        format!("HELO {}\r\n", server.server_name)
    };
    if write_client(upstream, command.as_bytes(), server.timeout)
        .await
        .is_err()
        || read_smtp_response(upstream, server).await.is_none()
    {
        upstream_error(client, server, logger, "SMTP EHLO").await;
        return false;
    }
    if server.proxy.xclient {
        let address = context
            .proxy_addresses
            .proxy_protocol_addr
            .map(|address| address.to_string())
            .or_else(|| {
                context
                    .remote
                    .socket_addr()
                    .map(|address| address.ip().to_string())
            })
            .unwrap_or_default();
        let mut xclient = format!("XCLIENT ADDR={address}");
        if !auth.user.is_empty() && !server.proxy.smtp_auth {
            xclient.push_str(" LOGIN=");
            xclient.push_str(&String::from_utf8_lossy(&auth.user));
        }
        xclient.push_str(" NAME=");
        xclient.push_str(&server.server_name);
        xclient.push_str("\r\n");
        if write_client(upstream, xclient.as_bytes(), server.timeout)
            .await
            .is_err()
            || read_smtp_response(upstream, server).await.is_none()
        {
            upstream_error(client, server, logger, "SMTP XCLIENT").await;
            return false;
        }
    }
    if !auth.smtp_helo.is_empty() {
        let command = if esmtp {
            let mut command = b"EHLO ".to_vec();
            command.extend_from_slice(&auth.smtp_helo);
            command.extend_from_slice(b"\r\n");
            command
        } else {
            let mut command = b"HELO ".to_vec();
            command.extend_from_slice(&auth.smtp_helo);
            command.extend_from_slice(b"\r\n");
            command
        };
        if write_client(upstream, &command, server.timeout)
            .await
            .is_err()
            || read_smtp_response(upstream, server).await.is_none()
        {
            upstream_error(client, server, logger, "SMTP client EHLO").await;
            return false;
        }
    }
    if auth.method != MailAuthMethod::None {
        if server.proxy.smtp_auth {
            if !auth.password_available {
                upstream_error(client, server, logger, "SMTP AUTH password").await;
                return false;
            }
            let mut plain = vec![0];
            plain.extend_from_slice(&auth.user);
            plain.push(0);
            plain.extend_from_slice(&auth.password);
            let encoded = base64::engine::general_purpose::STANDARD.encode(plain);
            let command = format!("AUTH PLAIN {encoded}\r\n");
            if write_client(upstream, command.as_bytes(), server.timeout)
                .await
                .is_err()
            {
                upstream_error(client, server, logger, "SMTP AUTH").await;
                return false;
            }
            let Some(response) = read_smtp_response(upstream, server).await else {
                upstream_error(client, server, logger, "SMTP AUTH response").await;
                return false;
            };
            if !smtp_status_is(response.last_response_status(), b'2') {
                let _ = write_client(client, &response.bytes, server.timeout).await;
                return false;
            }
        }
        if write_client(client, b"235 2.0.0 OK\r\n", server.timeout)
            .await
            .is_err()
        {
            return false;
        }
    } else {
        if write_client(upstream, &auth.smtp_from, server.timeout)
            .await
            .is_err()
            || write_client(upstream, b"\r\n", server.timeout)
                .await
                .is_err()
            || read_smtp_response(upstream, server).await.is_none()
        {
            upstream_error(client, server, logger, "SMTP MAIL FROM").await;
            return false;
        }
        if write_client(upstream, &auth.smtp_to, server.timeout)
            .await
            .is_err()
            || write_client(upstream, b"\r\n", server.timeout)
                .await
                .is_err()
        {
            upstream_error(client, server, logger, "SMTP RCPT TO").await;
            return false;
        }
        let Some(response) = read_smtp_response(upstream, server).await else {
            upstream_error(client, server, logger, "SMTP RCPT TO response").await;
            return false;
        };
        let _ = write_client(client, &response.bytes, server.timeout).await;
    }
    logger.log(
        LogLevel::Info,
        format!("mail client {} logged in", context.remote),
    );
    true
}

struct SmtpResponse {
    bytes: Vec<u8>,
}

impl SmtpResponse {
    fn last_response_status(&self) -> u8 {
        self.bytes.first().copied().unwrap_or(b'5')
    }
}

async fn read_smtp_response(
    upstream: &mut MailReader,
    server: &MailServer,
) -> Option<SmtpResponse> {
    let mut bytes = Vec::new();
    loop {
        let line = read_upstream_line(upstream, server).await?;
        let done = line.len() < 4 || line[3] != b'-';
        bytes.extend_from_slice(&line);
        if done {
            return Some(SmtpResponse { bytes });
        }
    }
}

fn smtp_status_is(status: u8, expected: u8) -> bool {
    status == expected
}

async fn read_upstream_line(upstream: &mut MailReader, server: &MailServer) -> Option<Vec<u8>> {
    match read_line(upstream, server.proxy.buffer_size, server.timeout).await {
        Ok(line) => line,
        Err(_) => None,
    }
}

async fn upstream_error(
    client: &mut MailReader,
    server: &MailServer,
    logger: &Logger,
    action: &str,
) {
    logger.log(
        LogLevel::Error,
        format!("mail upstream error during {action}"),
    );
    let response = mail_internal_error(server.protocol);
    let _ = write_client(client, response, server.timeout).await;
}

fn mail_internal_error(protocol: MailProtocol) -> &'static [u8] {
    match protocol {
        MailProtocol::Pop3 => b"-ERR internal server error\r\n",
        MailProtocol::Imap => b"* BAD internal server error\r\n",
        MailProtocol::Smtp => b"451 4.3.2 Internal server error\r\n",
    }
}

async fn write_mail_proxy_protocol<W>(
    outbound: &mut W,
    version: MailProxyProtocol,
    source: &ListenAddress,
    destination: &ListenAddress,
    timeout: Duration,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let header = match version {
        MailProxyProtocol::Off => return Ok(()),
        MailProxyProtocol::On => {
            let (ListenAddress::Inet(source), ListenAddress::Inet(destination)) =
                (source, destination)
            else {
                return write_timeout(outbound, b"PROXY UNKNOWN\r\n", timeout).await;
            };
            let family = match (source.ip(), destination.ip()) {
                (IpAddr::V4(_), IpAddr::V4(_)) => "TCP4",
                (IpAddr::V6(_), IpAddr::V6(_)) => "TCP6",
                _ => return write_timeout(outbound, b"PROXY UNKNOWN\r\n", timeout).await,
            };
            format!(
                "PROXY {family} {} {} {} {}\r\n",
                source.ip(),
                destination.ip(),
                source.port(),
                destination.port()
            )
            .into_bytes()
        }
        MailProxyProtocol::V2 => build_mail_proxy_v2(source, destination),
    };
    write_timeout(outbound, &header, timeout).await
}

async fn write_timeout<W>(outbound: &mut W, data: &[u8], timeout: Duration) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    match tokio::time::timeout(timeout, async {
        outbound.write_all(data).await?;
        outbound.flush().await
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "mail upstream write timed out",
        )),
    }
}

fn build_mail_proxy_v2(source: &ListenAddress, destination: &ListenAddress) -> Vec<u8> {
    let mut header = b"\r\n\r\n\0\r\nQUIT\n".to_vec();
    let (source, destination) = match (source, destination) {
        (ListenAddress::Inet(source), ListenAddress::Inet(destination)) => (*source, *destination),
        _ => {
            header.extend_from_slice(&[0x20, 0x00, 0x00, 0x00]);
            return header;
        }
    };
    if source.is_ipv4() && destination.is_ipv4() {
        header.extend_from_slice(&[0x21, 0x11, 0x00, 0x0c]);
        if let (IpAddr::V4(source_ip), IpAddr::V4(destination_ip)) = (source.ip(), destination.ip())
        {
            header.extend_from_slice(&source_ip.octets());
            header.extend_from_slice(&destination_ip.octets());
        }
    } else {
        header.extend_from_slice(&[0x21, 0x21, 0x00, 0x24]);
        header.extend_from_slice(&ipv6_octets(source.ip()));
        header.extend_from_slice(&ipv6_octets(destination.ip()));
    }
    header.extend_from_slice(&source.port().to_be_bytes());
    header.extend_from_slice(&destination.port().to_be_bytes());
    header
}

fn effective_mail_remote(server: &MailServer, addresses: &ProxyProtocolAddresses) -> ListenAddress {
    if addresses.proxy_protocol_addr.is_none() || server.real_ip_from.is_empty() {
        return addresses.transport_remote.clone();
    }
    let trusted = match &addresses.transport_remote {
        ListenAddress::Inet(address) => {
            let peer = normalize_mail_real_ip(address.ip());
            server
                .real_ip_from
                .iter()
                .any(|source| source.contains(peer))
        }
        ListenAddress::Unix(_) => server
            .real_ip_from
            .iter()
            .any(crate::config::RealIpSource::is_unix),
    };
    if trusted {
        addresses.remote.clone()
    } else {
        addresses.transport_remote.clone()
    }
}

fn normalize_mail_real_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(address)),
        address => address,
    }
}

fn ipv6_octets(address: IpAddr) -> [u8; 16] {
    match address {
        IpAddr::V4(address) => address.to_ipv6_mapped().octets(),
        IpAddr::V6(address) => address.octets(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_mail_proxy_v1_header() {
        let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(
            "127.0.0.1:1234".parse().unwrap(),
        ));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut output = Vec::new();
        runtime
            .block_on(write_mail_proxy_protocol(
                &mut output,
                MailProxyProtocol::On,
                &addresses.transport_remote,
                &addresses.transport_remote,
                Duration::from_secs(1),
            ))
            .unwrap();
        assert_eq!(output, b"PROXY TCP4 127.0.0.1 127.0.0.1 1234 1234\r\n");
    }
}
