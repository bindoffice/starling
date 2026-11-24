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
    match write_dispatch_response(write_half, outcome, request, control).await {
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
