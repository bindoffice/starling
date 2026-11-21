use super::*;

pub(super) struct OpenedTunnel {
    pub(super) io: BoxedUpstreamIo,
    pub(super) guard: Option<UpstreamPeerGuard>,
}

pub(super) async fn open(
    targets: &[ResolvedUpstreamTarget],
    dispatch: &TunnelDispatch,
) -> Result<OpenedTunnel, u16> {
    let started = Instant::now();
    let mut attempts = 0_usize;
    let mut last_status = 502;
    for target in targets {
        if dispatch.next_upstream.tries != 0 && attempts >= dispatch.next_upstream.tries {
            break;
        }
        if dispatch.next_upstream.timeout != Duration::ZERO
            && started.elapsed() >= dispatch.next_upstream.timeout
        {
            return Err(504);
        }
        attempts = attempts.saturating_add(1);
        let mut guard = match target.try_acquire() {
            Ok(guard) => guard,
            Err(()) => continue,
        };
        let result = open_gateway_io(
            &target.host,
            target.port,
            dispatch.connect_timeout,
            dispatch.socket_options,
            dispatch.bind.as_deref(),
            "tunnel_bind",
        )
        .await;
        match result {
            Ok(io) => return Ok(OpenedTunnel { io, guard }),
            Err(ProxyFailure::Timeout) => {
                last_status = 504;
                if let Some(guard) = guard.as_mut() {
                    guard.mark_failed();
                }
                if !dispatch.next_upstream.retry_timeout() {
                    break;
                }
            }
            Err(_) => {
                last_status = 502;
                if let Some(guard) = guard.as_mut() {
                    guard.mark_failed();
                }
                if !dispatch.next_upstream.retry_error() {
                    break;
                }
            }
        }
    }
    Err(last_status)
}
