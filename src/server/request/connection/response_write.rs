use super::*;

pub(super) enum DispatchWrite {
    Written,
    ClientClosed,
    Failed(io::Error),
}

pub(super) async fn write_dispatch_response<W>(
    write_half: &mut W,
    outcome: &mut DispatchOutcome,
    request: &Request,
    control: Option<&ClientSocketControl>,
) -> DispatchWrite
where
    W: AsyncWrite + Unpin,
{
    let mut writer = TimedWriter::new(write_half, outcome.send_timeout);
    let options = ResponseWriteOptions {
        request_method: &request.method,
        request_version: &request.version,
        keep_alive: outcome.keep_alive,
        keep_alive_header: outcome.keepalive_header,
        server_tokens: outcome.server_tokens,
        chunked_transfer_encoding: outcome.chunked_transfer_encoding,
        rate: outcome.limit_rate,
    };
    let chunked = if outcome.response_prepared {
        outcome.response_chunked
    } else {
        prepare_response_for_write(&mut outcome.response, &options)
    };
    let write = write_prepared_response(&mut writer, &mut outcome.response, options, chunked);
    tokio::pin!(write);
    let result = if let Some(control) = control {
        tokio::select! {
            result = &mut write => Some(result),
            _ = wait_for_client_reset(control) => None,
        }
    } else {
        Some(write.await)
    };
    match result {
        None => DispatchWrite::ClientClosed,
        Some(Ok(())) => DispatchWrite::Written,
        Some(Err(error)) => DispatchWrite::Failed(error),
    }
}
