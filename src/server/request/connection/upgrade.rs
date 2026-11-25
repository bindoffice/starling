use super::*;
use crate::server::request::tunnel::{
    BidirectionalCopyStats, BidirectionalCopyTimeouts, copy_bidirectional,
};

pub(super) enum UpgradeOutcome {
    ClientClosed,
    Finished {
        result: io::Result<()>,
        bytes_sent: u64,
        body_bytes_sent: u64,
    },
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_proxy_upgrade<S, W>(
    body_task: &mut Option<Http1BodyTask<S>>,
    reader: &mut Option<BufReader<tokio::io::ReadHalf<S>>>,
    write_half: &mut W,
    outcome: &mut DispatchOutcome,
    proxy_upgrade: ProxyUpgrade,
    request: &Request,
    control: Option<&ClientSocketControl>,
    request_logger: &Logger,
    remote: &ListenAddress,
) where
    S: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Unpin,
{
    if let Some(task) = body_task.take() {
        task.close().await;
        match task.finish().await {
            Ok((restored, Ok(()))) => *reader = Some(restored),
            Ok((_, Err(err))) => {
                request_logger.log(
                    LogLevel::Info,
                    format!(
                        "client request body for \"{} {}\" from {remote} failed: {err}",
                        request.method, request.target
                    ),
                );
                return;
            }
            Err(err) => {
                request_logger.log(
                    LogLevel::Error,
                    format!(
                        "client request body task for \"{} {}\" failed: {err}",
                        request.method, request.target
                    ),
                );
                return;
            }
        }
    }
    let Some(reader) = reader.take() else {
        return;
    };
    if let Some(control) = control
        && let Err(err) = control.apply_nodelay_once(outcome.tcp_nodelay)
    {
        request_logger.log(
            LogLevel::Alert,
            format!("setsockopt(TCP_NODELAY) failed: {err}"),
        );
        return;
    }
    match serve_proxy_upgrade(reader, write_half, outcome, proxy_upgrade, request, control).await {
        UpgradeOutcome::ClientClosed => {}
        UpgradeOutcome::Finished {
            result,
            bytes_sent,
            body_bytes_sent,
        } => {
            if let Some(log) = outcome.deferred_access_log.take() {
                log.write_upgraded(bytes_sent, body_bytes_sent);
            }
            if let Err(err) = result {
                request_logger.log(
                    LogLevel::Info,
                    format!("proxied upgraded connection from {remote} failed: {err}"),
                );
            }
        }
    }
}

pub(super) async fn serve_proxy_upgrade<S, W>(
    reader: BufReader<tokio::io::ReadHalf<S>>,
    write_half: &mut W,
    outcome: &mut DispatchOutcome,
    upgrade: ProxyUpgrade,
    request: &Request,
    control: Option<&ClientSocketControl>,
) -> UpgradeOutcome
where
    S: AsyncRead + Send + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut scratch = Vec::with_capacity(512);
    match write_dispatch_response(write_half, outcome, request, control, &mut scratch).await {
        DispatchWrite::ClientClosed => return UpgradeOutcome::ClientClosed,
        DispatchWrite::Failed(error) => {
            return UpgradeOutcome::Finished {
                result: Err(error),
                bytes_sent: 0,
                body_bytes_sent: 0,
            };
        }
        DispatchWrite::Written => {}
    }
    let header_bytes = response_head_length(&outcome.response);

    let ProxyUpgrade {
        connection,
        read_timeout,
        write_timeout,
        buffer_size,
    } = upgrade;
    let downstream_prefix = reader.buffer().to_vec();
    let mut downstream = PrefixedStream::new(reader.into_inner(), downstream_prefix);
    let upstream_reader = connection.reader;
    let upstream_prefix = upstream_reader.buffer().to_vec();
    let upstream = PrefixedStream::new(upstream_reader.into_inner(), upstream_prefix);
    let mut stats = BidirectionalCopyStats::default();
    let result = copy_bidirectional(
        &mut downstream,
        write_half,
        upstream,
        BidirectionalCopyTimeouts {
            upstream_write: write_timeout,
            upstream_read: read_timeout,
            downstream_write: outcome.send_timeout,
            buffer_size,
        },
        &mut stats,
    )
    .await;
    UpgradeOutcome::Finished {
        result,
        bytes_sent: header_bytes.saturating_add(stats.upstream_to_downstream),
        body_bytes_sent: stats.upstream_to_downstream,
    }
}
