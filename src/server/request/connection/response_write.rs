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
    scratch: &mut Vec<u8>,
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
    let sendfile = outcome.sendfile
        && !chunked
        && options.rate.rate == 0
        && !request.method.eq_ignore_ascii_case("HEAD")
        && control.is_some_and(ClientSocketControl::supports_sendfile)
        && matches!(outcome.response.body, ResponseBody::File { .. });
    let write = async {
        if !sendfile {
            return write_prepared_response_with_scratch(
                &mut writer,
                &mut outcome.response,
                options,
                chunked,
                scratch,
            )
            .await;
        }
        let ResponseBody::File {
            file,
            offset,
            length,
        } = std::mem::replace(&mut outcome.response.body, ResponseBody::Empty)
        else {
            unreachable!("sendfile response body was checked above");
        };
        write_prepared_response_head(
            &mut writer,
            &outcome.response,
            options.request_version,
            scratch,
        )
        .await?;
        control
            .expect("sendfile requires client socket control")
            .sendfile(&file, offset, length, outcome.send_timeout)
            .await
    };
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
