use std::io;
use std::time::Duration;

#[cfg(unix)]
use std::os::fd::AsRawFd;

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

    #[cfg(unix)]
    if let Some(stream) = connection.reader.get_mut().as_tcp_stream() {
        let file = body_file
            .open_sync()
            .map_err(crate::http::request_body_file_error)?;
        sendfile_request_body(stream, &file, body_file.len(), context.proxy_send_timeout)
            .await
            .map_err(|error| {
                if error.kind() == io::ErrorKind::UnexpectedEof {
                    crate::http::request_body_file_error(error)
                } else {
                    error
                }
            })?;
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

#[cfg(unix)]
async fn sendfile_request_body(
    stream: &mut tokio::net::TcpStream,
    file: &std::fs::File,
    length: u64,
    timeout: Duration,
) -> io::Result<()> {
    let output_fd = stream.as_raw_fd();
    let input_fd = file.as_raw_fd();
    let mut offset = 0_u64;
    while offset < length {
        let chunk = (length - offset).min(1024 * 1024);
        let send = stream.async_io(tokio::io::Interest::WRITABLE, || {
            let mut file_offset = i64::try_from(offset).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "file offset exceeds i64")
            })?;
            let result = unsafe {
                libc::sendfile(
                    output_fd,
                    input_fd,
                    &mut file_offset,
                    chunk.min(usize::MAX as u64) as usize,
                )
            };
            if result > 0 {
                Ok(result as usize)
            } else if result == 0 {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "request body file ended before its recorded length",
                ))
            } else {
                Err(io::Error::last_os_error())
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
