use std::io;
#[cfg(target_os = "linux")]
use std::time::Duration;

use super::super::*;
use super::write::{proxy_request_head, write_remaining_proxy_body};
use crate::http::RequestBodyStream;

pub(super) async fn write_proxy_request_to_upstream(
    connection: &mut UpstreamConnection,
    outgoing: &ProxyRequestMessage,
    version: &str,
    context: &EffectiveConfig,
) -> io::Result<()> {
    let body_file = outgoing
        .body_stream
        .as_ref()
        .filter(|_| !outgoing.body_chunked)
        .and_then(RequestBodyStream::file);
    let Some(body_file) = body_file else {
        return super::write::write_proxy_request_timed(
            &mut connection.reader,
            outgoing,
            version,
            Some(context.proxy_send_timeout),
        )
        .await;
    };

    let head = proxy_request_head(outgoing, version);
    write_all_with_timeout(&mut connection.reader, &head, context.proxy_send_timeout).await?;

    #[cfg(target_os = "linux")]
    if let Some(stream) = connection.reader.get_mut().as_tcp_stream() {
        let file = body_file
            .open_sync()
            .map_err(crate::http::request_body_file_error)?;
        sendfile_request_body(stream, &file, body_file.len(), context.proxy_send_timeout)
            .await
            .map_err(classify_request_body_send_error)?;
        return flush_with_timeout(&mut connection.reader, context.proxy_send_timeout).await;
    }

    write_remaining_proxy_body(
        &mut connection.reader,
        outgoing,
        0,
        false,
        Some(context.proxy_send_timeout),
    )
    .await
}

/// Map a `sendfile` failure onto the correct error domain.
///
/// `sendfile` cannot report whether a failure came from reading the source
/// file or writing the socket, so distinguish them by errno. A closed or reset
/// upstream is a gateway failure (502/504); anything else is a local
/// request-body file failure that nginx surfaces as a 500.
#[cfg(target_os = "linux")]
fn classify_request_body_send_error(error: io::Error) -> io::Error {
    let upstream_side = error.kind() == io::ErrorKind::TimedOut
        || matches!(
            error.raw_os_error(),
            Some(libc::EPIPE)
                | Some(libc::ECONNRESET)
                | Some(libc::ECONNABORTED)
                | Some(libc::ENOTCONN)
                | Some(libc::ESHUTDOWN)
                | Some(libc::EPROTOTYPE)
        );
    if upstream_side {
        error
    } else {
        crate::http::request_body_file_error(error)
    }
}

#[cfg(target_os = "linux")]
async fn sendfile_request_body(
    stream: &mut tokio::net::TcpStream,
    file: &std::fs::File,
    length: u64,
    timeout: Duration,
) -> io::Result<()> {
    let mut offset = 0_u64;
    while offset < length {
        let chunk = (length - offset).min(1024 * 1024);
        let send = stream.async_io(tokio::io::Interest::WRITABLE, || {
            let mut file_offset = offset;
            match rustix::fs::sendfile(
                &*stream,
                file,
                Some(&mut file_offset),
                chunk.min(usize::MAX as u64) as usize,
            ) {
                Ok(0) => Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "request body file ended before its recorded length",
                )),
                Ok(sent) => Ok(sent),
                Err(error) => Err(error.into()),
            }
        });
        let sent = if timeout == Duration::ZERO {
            send.await
        } else {
            tokio::time::timeout(timeout, send)
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "sendfile timed out"))?
        };
        match sent {
            Ok(sent) => offset = offset.saturating_add(sent as u64),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
