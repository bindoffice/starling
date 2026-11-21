use super::*;

pub(in crate::server) struct GrpcAttemptResult {
    pub(in crate::server) response: Response,
    pub(in crate::server) reusable: Option<ReusableUpstreamConnection>,
    pub(in crate::server) timing: UpstreamAttemptTiming,
}

pub(in crate::server) async fn grpc_attempt(
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    variables: &Variables,
    keepalive: Option<&UpstreamKeepaliveContext>,
    target: GrpcAttemptTarget<'_>,
) -> Result<GrpcAttemptResult, ProxyFailure> {
    let GrpcAttemptTarget {
        host,
        port,
        tls,
        authority,
    } = target;
    let outgoing = build_grpc_request(request, env, context, variables, authority, tls)?;
    let started = Instant::now();
    let pool_key = keepalive.map(|_| grpc_upstream_pool_key(host, port, tls, context, variables));
    let mut connection = match (keepalive, pool_key.as_ref()) {
        (Some(keepalive), Some(key)) => {
            variables
                .upstream_keepalive
                .take_http2(&keepalive.name, key, &keepalive.config)
                .await
        }
        _ => None,
    };
    if connection.is_none() {
        let key = pool_key
            .clone()
            .unwrap_or_else(|| grpc_upstream_pool_key(host, port, tls, context, variables));
        let io = open_grpc_upstream(host, port, tls, context, variables).await?;
        connection = Some(ProxyV2Connection::connect(io, key, context.grpc.connect_timeout).await?);
    }
    let mut connection = connection.expect("gRPC HTTP/2 connection");
    let connect = started.elapsed();
    let mut buffer_options = crate::http::UpstreamBodyBufferOptions::memory(MAX_BODY_BYTES);
    buffer_options.write_size = context.grpc.buffer_size;
    let (upstream_response, header_time) = timed_h2_exchange(
        &mut connection,
        outgoing.request,
        &outgoing.body,
        outgoing.body_stream.as_ref(),
        context.client_max_body_size,
        H2ExchangeOptions {
            send_timeout: context.grpc.send_timeout,
            read_timeout: context.grpc.read_timeout,
            buffer_policy: UpstreamBodyBufferPolicy::fixed(buffer_options),
            limit_rate: 0,
            streaming: Some(H2StreamingPolicy::Grpc),
        },
    )
    .await?;
    let reusable = match (keepalive, pool_key) {
        (Some(keepalive), Some(_)) if connection.idle_is_open() => {
            Some(ReusableUpstreamConnection::Http2 {
                upstream: keepalive.name.clone(),
                config: keepalive.config.clone(),
                connection,
            })
        }
        _ => None,
    };
    let header = connect.saturating_add(header_time);
    let response = finalize_grpc_response(upstream_response, context);
    Ok(GrpcAttemptResult {
        response,
        reusable,
        timing: UpstreamAttemptTiming {
            connect,
            first_byte: Some(header),
            header,
            response: started.elapsed(),
        },
    })
}

#[derive(Clone, Copy)]
pub(in crate::server) struct GrpcAttemptTarget<'a> {
    pub(in crate::server) host: &'a str,
    pub(in crate::server) port: u16,
    pub(in crate::server) tls: bool,
    pub(in crate::server) authority: &'a str,
}
