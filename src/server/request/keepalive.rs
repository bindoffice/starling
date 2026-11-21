use super::*;
use tokio::io::AsyncBufReadExt;

#[derive(Clone, Copy)]
pub(in crate::server) struct ConnectionRequestState {
    pub(in crate::server) started: Instant,
    pub(in crate::server) served_requests: u64,
}

impl ConnectionRequestState {
    pub(in crate::server) fn new(started: Instant, served_requests: u64) -> Self {
        Self {
            started,
            served_requests,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct KeepalivePolicy {
    timeout: Duration,
    min_timeout: Duration,
}

impl KeepalivePolicy {
    pub(super) fn new(timeout: Duration, min_timeout: Duration) -> Self {
        Self {
            timeout,
            min_timeout,
        }
    }

    pub(super) fn initial_idle_timeout(self) -> Duration {
        if self.min_timeout == Duration::ZERO {
            Duration::ZERO
        } else if self.timeout > self.min_timeout {
            self.min_timeout
        } else {
            self.timeout
        }
    }

    pub(super) fn remaining_idle_timeout(self) -> Duration {
        self.timeout.saturating_sub(self.initial_idle_timeout())
    }

    pub(super) fn becomes_reusable(self) -> bool {
        self.min_timeout == Duration::ZERO || self.timeout > self.min_timeout
    }
}

pub(super) enum KeepaliveWait {
    Data,
    Closed,
    TimedOut,
}

pub(super) enum KeepaliveWaitOrShutdown {
    Input(io::Result<KeepaliveWait>),
    Shutdown,
}

pub(super) async fn wait_for_keepalive_input<R>(
    reader: &mut R,
    timeout: Duration,
) -> io::Result<KeepaliveWait>
where
    R: AsyncBufRead + Unpin,
{
    if timeout == Duration::ZERO {
        return Ok(KeepaliveWait::TimedOut);
    }
    match tokio::time::timeout(timeout, reader.fill_buf()).await {
        Ok(Ok([])) => Ok(KeepaliveWait::Closed),
        Ok(Ok(_)) => Ok(KeepaliveWait::Data),
        Ok(Err(err)) => Err(err),
        Err(_) => Ok(KeepaliveWait::TimedOut),
    }
}

pub(super) async fn wait_for_keepalive_input_or_shutdown<R>(
    reader: &mut R,
    timeout: Duration,
    shutdown: &mut Option<watch::Receiver<ShutdownMode>>,
) -> KeepaliveWaitOrShutdown
where
    R: AsyncBufRead + Unpin,
{
    let Some(shutdown) = shutdown.as_mut() else {
        return KeepaliveWaitOrShutdown::Input(wait_for_keepalive_input(reader, timeout).await);
    };
    tokio::select! {
        result = wait_for_keepalive_input(reader, timeout) => {
            KeepaliveWaitOrShutdown::Input(result)
        }
        _ = wait_for_shutdown(shutdown) => KeepaliveWaitOrShutdown::Shutdown,
    }
}

pub(super) fn keepalive_disabled(context: &EffectiveConfig, request: &Request) -> bool {
    let user_agent = request.header("user-agent").unwrap_or_default();
    (context.keepalive_disable.msie6
        && request.method == "POST"
        && super::super::user_agent::is_msie6(user_agent))
        || (context.keepalive_disable.safari && super::super::user_agent::is_safari(user_agent))
}

pub(in crate::server) fn should_keep_alive(
    request: &Request,
    context: &EffectiveConfig,
    response: &Response,
    force_close: bool,
    connection: ConnectionRequestState,
) -> bool {
    let unsized_stream = response.body.is_stream() && response.body.content_length().is_none();
    !force_close
        && !keepalive_disabled(context, request)
        && request.keep_alive
        && !(unsized_stream
            && (request.version != "HTTP/1.1" || !context.chunked_transfer_encoding))
        && response.status < 500
        && context.keepalive_timeout > Duration::ZERO
        && (context.keepalive_time == Duration::ZERO
            || connection.started.elapsed() < context.keepalive_time)
        && connection.served_requests < context.keepalive_requests
}

pub(in crate::server) fn keepalive_header_timeout(
    context: &EffectiveConfig,
    keep_alive: bool,
) -> Option<u64> {
    keep_alive
        .then_some(context.keepalive_header)
        .flatten()
        .filter(|timeout| !timeout.is_zero())
        .map(|timeout| timeout.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_min_timeout_delays_reusable_state() {
        let immediate = KeepalivePolicy::new(Duration::from_secs(5), Duration::ZERO);
        assert_eq!(immediate.initial_idle_timeout(), Duration::ZERO);
        assert_eq!(immediate.remaining_idle_timeout(), Duration::from_secs(5));
        assert!(immediate.becomes_reusable());

        let delayed = KeepalivePolicy::new(Duration::from_secs(5), Duration::from_secs(2));
        assert_eq!(delayed.initial_idle_timeout(), Duration::from_secs(2));
        assert_eq!(delayed.remaining_idle_timeout(), Duration::from_secs(3));
        assert!(delayed.becomes_reusable());

        let never_reusable = KeepalivePolicy::new(Duration::from_secs(2), Duration::from_secs(5));
        assert_eq!(
            never_reusable.initial_idle_timeout(),
            Duration::from_secs(2)
        );
        assert_eq!(never_reusable.remaining_idle_timeout(), Duration::ZERO);
        assert!(!never_reusable.becomes_reusable());
    }
}
