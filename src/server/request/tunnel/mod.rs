use super::*;

mod connect;
mod relay;

pub(in crate::server) struct TunnelDispatch {
    pub(in crate::server) response: Response,
    pub(in crate::server) error_logger: Option<Arc<Logger>>,
    pub(in crate::server) targets: Vec<ResolvedUpstreamTarget>,
    pub(super) bind: Option<String>,
    pub(super) socket_options: UpstreamSocketOptions,
    pub(super) connect_timeout: Duration,
    pub(super) upstream_send_timeout: Duration,
    pub(super) upstream_read_timeout: Duration,
    pub(super) downstream_send_timeout: Duration,
    pub(super) buffer_size: usize,
    pub(super) next_upstream: NextUpstreamConfig,
    pub(super) server_tokens: ServerTokens,
    pub(super) tcp_nodelay: bool,
}

pub(in crate::server) fn build_tunnel_dispatch(
    http: &HttpConfig,
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> Result<TunnelDispatch, Box<Response>> {
    let pass = context
        .tunnel
        .pass
        .as_ref()
        .expect("tunnel dispatch requires tunnel_pass");
    let value = eval_variables(&pass.raw, variables);
    if value.is_empty() || value.contains("://") {
        return Err(Box::new(error_response(500)));
    }
    let targets = if let Some(upstream) = http.upstreams.get(&value) {
        let hash_key = upstream
            .hash_key
            .as_deref()
            .map(|key| eval_variables(key, variables));
        resolve_upstream_config(upstream, request_peer_of(variables), hash_key.as_deref())
    } else {
        parse_upstream_address(&value)
            .map(|(host, port)| {
                vec![ResolvedUpstreamTarget {
                    host,
                    port,
                    peer: None,
                    check_health: false,
                }]
            })
            .unwrap_or_default()
    };
    if targets.is_empty() {
        return Err(Box::new(error_response(502)));
    }
    variables.upstream_addr = targets
        .iter()
        .map(|target| {
            if target.host.starts_with("unix:") {
                target.host.clone()
            } else {
                format!("{}:{}", target.host, target.port)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    Ok(TunnelDispatch {
        response: Response::new(200),
        error_logger: None,
        targets,
        bind: evaluate_upstream_bind(context.tunnel.bind.as_deref(), variables),
        socket_options: UpstreamSocketOptions::for_tunnel(context),
        connect_timeout: context.tunnel.connect_timeout,
        upstream_send_timeout: context.tunnel.send_timeout,
        upstream_read_timeout: context.tunnel.read_timeout,
        downstream_send_timeout: context.send_timeout,
        buffer_size: context.tunnel.buffer_size,
        next_upstream: context.tunnel.next_upstream,
        server_tokens: context.server_tokens,
        tcp_nodelay: context.tcp_nodelay,
    })
}

pub(in crate::server) async fn serve_tunnel<R, W>(
    downstream_reader: &mut R,
    downstream_writer: &mut W,
    dispatch: TunnelDispatch,
    logger: &Logger,
    remote: &ListenAddress,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let opened = match connect::open(&dispatch.targets, &dispatch).await {
        Ok(opened) => opened,
        Err(status) => {
            let mut response = error_response(status);
            return write_response(
                downstream_writer,
                &mut response,
                ResponseWriteOptions {
                    request_method: "CONNECT",
                    request_version: "HTTP/1.1",
                    keep_alive: false,
                    keep_alive_header: None,
                    server_tokens: dispatch.server_tokens,
                    chunked_transfer_encoding: true,
                    rate: ResponseRate::default(),
                },
            )
            .await;
        }
    };
    let mut response = dispatch.response;
    relay::write_tunnel_response(downstream_writer, &mut response, dispatch.server_tokens).await?;
    let result = relay::copy_bidirectional(
        downstream_reader,
        downstream_writer,
        opened.io,
        dispatch.upstream_send_timeout,
        dispatch.upstream_read_timeout,
        dispatch.downstream_send_timeout,
        dispatch.buffer_size,
    )
    .await;
    drop(opened.guard);
    if let Err(error) = &result {
        logger.log(
            LogLevel::Info,
            format!("http tunnel for {remote} failed: {error}"),
        );
    }
    result
}

fn request_peer_of(variables: &Variables) -> Option<IpAddr> {
    variables
        .remote_addr
        .parse::<SocketAddr>()
        .ok()
        .map(|address| address.ip())
}
