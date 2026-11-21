use super::*;

mod connection;
mod exchange;
mod headers;
mod request;
mod response;
mod streaming;
mod streaming_prefetch;

pub(in crate::server) use connection::ProxyV2Connection;
pub(in crate::server) use exchange::{H2ExchangeOptions, H2StreamingPolicy, timed_h2_exchange};
use request::build_h2_request;

pub(in crate::server) async fn timed_proxy_v2_exchange(
    connection: &mut ProxyV2Connection,
    outgoing: &ProxyRequestMessage,
    proxy: &ProxyPassConfig,
    context: &EffectiveConfig,
    limit_rate: u64,
    streaming_allowed: bool,
) -> Result<(UpstreamResponse, Duration), ProxyFailure> {
    let request = build_h2_request(outgoing, proxy)?;
    timed_h2_exchange(
        connection,
        request,
        &outgoing.body,
        outgoing.body_stream.as_ref(),
        outgoing.body_limit,
        H2ExchangeOptions {
            send_timeout: context.proxy_send_timeout,
            read_timeout: context.proxy_read_timeout,
            buffer_policy: UpstreamBodyBufferPolicy::upstream(
                &context.proxy_buffering,
                !ignores_accel_buffering(&context.proxy_ignore_headers),
            ),
            limit_rate,
            streaming: (streaming_allowed && !outgoing.method.eq_ignore_ascii_case("HEAD"))
                .then_some(H2StreamingPolicy::Proxy(context)),
        },
    )
    .await
}
