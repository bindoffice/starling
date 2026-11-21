use super::*;

mod attempt;
mod request;
mod response;
mod target;
mod transport;

use attempt::{GrpcAttemptTarget, grpc_attempt};
pub(in crate::server) use request::build_grpc_request;
pub(in crate::server) use response::*;
use target::resolve_grpc_targets;
pub(in crate::server) use transport::grpc_upstream_pool_key;
pub(in crate::server) use transport::open_grpc_upstream;

pub(in crate::server) async fn grpc_request(
    pass: &GrpcPassConfig,
    http: &HttpConfig,
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> Response {
    if let Some(client_abort) = variables.client_abort.as_ref() {
        client_abort.mark_upstream_active();
    }
    let streaming_request_body = request_body_stream_enabled(request, context);
    let selection = resolve_grpc_targets(pass, http, variables, request_peer(request));
    let keepalive = selection.upstream.as_ref().and_then(|name| {
        let upstream = http.upstreams.get(name)?;
        let config = upstream.keepalive.clone()?;
        (config.max_cached != 0).then(|| UpstreamKeepaliveContext {
            name: name.clone(),
            config,
        })
    });
    let mut attempted = Vec::new();
    let mut last_failure = ProxyFailure::Failure;
    let mut next = NextUpstreamState::new(&context.grpc.next_upstream);
    let mut targets = selection.targets.into_iter();
    let mut upstream_statuses = Vec::new();
    let mut upstream_response_times = Vec::new();
    while let Some(target) = targets.next() {
        let mut guard = match target.try_acquire() {
            Ok(guard) => guard,
            Err(()) => continue,
        };
        if !next.start_attempt() {
            break;
        }
        attempted.push(upstream_target_display(&target));
        let authority = selection.authority.clone();
        let mut upstream_variables = variables.clone();
        upstream_variables.proxy_host = authority.clone();
        upstream_variables.upstream_addr = upstream_target_display(&target);
        let started = Instant::now();
        match grpc_attempt(
            request,
            env,
            context,
            &upstream_variables,
            keepalive.as_ref(),
            GrpcAttemptTarget {
                host: &target.host,
                port: target.port,
                tls: selection.tls,
                authority: &authority,
            },
        )
        .await
        {
            Ok(result) => {
                upstream_statuses.push(result.response.status.to_string());
                upstream_response_times.push(format!("{:.3}", started.elapsed().as_secs_f64()));
                if !targets.as_slice().is_empty()
                    && !streaming_request_body
                    && next.can_retry_status(request, result.response.status)
                {
                    if let Some(guard) = guard.as_mut() {
                        guard.mark_failed();
                    }
                    continue;
                }
                if let Some(guard) = guard.as_mut() {
                    guard.record_timing(result.timing);
                }
                if let Some(reusable) = result.reusable {
                    match reusable {
                        ReusableUpstreamConnection::Http2 {
                            upstream,
                            config,
                            connection,
                        } => {
                            variables
                                .upstream_keepalive
                                .put_http2(&upstream, &config, connection)
                                .await;
                        }
                        ReusableUpstreamConnection::Http1 { .. } => {}
                    }
                }
                variables.upstream_addr = attempted.join(", ");
                variables.upstream_status = upstream_statuses.join(", ");
                variables.upstream_response_time = upstream_response_times.join(", ");
                return result.response;
            }
            Err(failure) => {
                upstream_statuses.push("-".into());
                upstream_response_times.push(format!("{:.3}", started.elapsed().as_secs_f64()));
                if let Some(guard) = guard.as_mut() {
                    guard.mark_failed();
                }
                last_failure = failure;
                if targets.as_slice().is_empty()
                    || streaming_request_body
                    || !next.can_retry_failure(request, &last_failure)
                {
                    break;
                }
            }
        }
    }
    variables.upstream_addr = attempted.join(", ");
    variables.upstream_status = upstream_statuses.join(", ");
    variables.upstream_response_time = upstream_response_times.join(", ");
    match last_failure {
        ProxyFailure::Timeout => error_response(504),
        ProxyFailure::Failure | ProxyFailure::InvalidHeader => error_response(502),
        ProxyFailure::RequestBody(status) => error_response(status),
    }
}
