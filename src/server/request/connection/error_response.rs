use super::*;

pub(super) async fn write_request_error_response<W>(
    write_half: &mut W,
    status: u16,
    send_timeout: Duration,
    request_method: &str,
    request_version: &str,
    server_tokens: ServerTokens,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut response = error_response(status);
    let mut writer = TimedWriter::new(write_half, send_timeout);
    write_response(
        &mut writer,
        &mut response,
        ResponseWriteOptions {
            request_method,
            request_version,
            keep_alive: false,
            keep_alive_header: None,
            server_tokens,
            chunked_transfer_encoding: true,
            rate: ResponseRate::default(),
        },
    )
    .await
}

pub(super) async fn write_request_rejection<W>(
    write_half: &mut W,
    status: u16,
    send_timeout: Duration,
    request_method: &str,
    request_version: &str,
    server_tokens: ServerTokens,
) where
    W: AsyncWrite + Unpin,
{
    let _ = write_request_error_response(
        write_half,
        status,
        send_timeout,
        request_method,
        request_version,
        server_tokens,
    )
    .await;
}
