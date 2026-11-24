use super::*;
use std::future::Future;

pub(super) async fn write_tunnel_response<W: AsyncWrite + Unpin>(
    writer: &mut W,
    response: &mut Response,
    server_tokens: ServerTokens,
) -> io::Result<()> {
    prepare_response_headers(response, server_tokens);
    response.set_header("Connection", "close");
    response.remove_header("Content-Length");
    response.remove_header("Transfer-Encoding");
    let mut head = format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason_phrase(response.status)
    );
    for header in &response.headers {
        head.push_str(&header.name);
        head.push_str(": ");
        head.push_str(&sanitize_header_value(&header.value));
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    writer.write_all(head.as_bytes()).await?;
    writer.flush().await
}

pub(super) async fn copy_bidirectional<R, W>(
    downstream_reader: &mut R,
    downstream_writer: &mut W,
    upstream: BoxedUpstreamIo,
    upstream_send_timeout: Duration,
    upstream_read_timeout: Duration,
    downstream_send_timeout: Duration,
    buffer_size: usize,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (mut upstream_reader, mut upstream_writer) = tokio::io::split(upstream);
    let upload = copy_direction(
        downstream_reader,
        &mut upstream_writer,
        None,
        upstream_send_timeout,
        buffer_size,
    );
    let download = copy_direction(
        &mut upstream_reader,
        downstream_writer,
        Some(upstream_read_timeout),
        downstream_send_timeout,
        buffer_size,
    );
    tokio::pin!(upload);
    tokio::pin!(download);
    let mut upload_done = false;
    let mut download_done = false;
    loop {
        if upload_done && download_done {
            return Ok(());
        }
        tokio::select! {
            result = &mut upload, if !upload_done => {
                upload_done = true;
                result?;
            }
            result = &mut download, if !download_done => {
                download_done = true;
                result?;
            }
        }
    }
}

async fn copy_direction<R, W>(
    reader: &mut R,
    writer: &mut W,
    read_timeout: Option<Duration>,
    write_timeout: Duration,
    buffer_size: usize,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0_u8; buffer_size];
    loop {
        let read = with_timeout(read_timeout, reader.read(&mut buffer)).await?;
        if read == 0 {
            // Propagate a clean half-close so the opposite direction can
            // finish without waiting for a full connection teardown.
            writer.shutdown().await?;
            return Ok(());
        }
        with_timeout(Some(write_timeout), writer.write_all(&buffer[..read])).await?;
    }
}

async fn with_timeout<T>(
    timeout: Option<Duration>,
    future: impl Future<Output = io::Result<T>>,
) -> io::Result<T> {
    let Some(timeout) = timeout.filter(|timeout| *timeout != Duration::ZERO) else {
        return future.await;
    };
    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "http tunnel I/O timed out",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn copies_both_directions_and_propagates_half_close() {
        let (mut client, downstream) = tokio::io::duplex(64 * 1024);
        let (mut upstream_client, upstream) = tokio::io::duplex(64 * 1024);
        let (mut reader, mut writer) = tokio::io::split(downstream);
        let task = tokio::spawn(async move {
            copy_bidirectional(
                &mut reader,
                &mut writer,
                Box::new(upstream),
                Duration::from_secs(2),
                Duration::from_secs(2),
                Duration::from_secs(2),
                4096,
            )
            .await
        });

        client.write_all(b"client-to-upstream").await.unwrap();
        let mut received = vec![0; 18];
        upstream_client.read_exact(&mut received).await.unwrap();
        assert_eq!(&received, b"client-to-upstream");
        client.shutdown().await.unwrap();

        upstream_client
            .write_all(b"upstream-to-client")
            .await
            .unwrap();
        upstream_client.shutdown().await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert_eq!(&response, b"upstream-to-client");

        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn preserves_prefetched_downstream_bytes() {
        let (mut client, downstream) = tokio::io::duplex(64 * 1024);
        let (mut upstream_client, upstream) = tokio::io::duplex(64 * 1024);
        let prefixed = crate::server::prefixed::PrefixedStream::new(
            downstream,
            b"prefetched-payload".to_vec(),
        );
        let (mut reader, mut writer) = tokio::io::split(prefixed);
        let task = tokio::spawn(async move {
            copy_bidirectional(
                &mut reader,
                &mut writer,
                Box::new(upstream),
                Duration::from_secs(2),
                Duration::from_secs(2),
                Duration::from_secs(2),
                4096,
            )
            .await
        });

        let mut received = vec![0; 18];
        upstream_client.read_exact(&mut received).await.unwrap();
        assert_eq!(&received, b"prefetched-payload");
        client.shutdown().await.unwrap();
        upstream_client.shutdown().await.unwrap();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn write_timeout_stops_a_stalled_tunnel_direction() {
        let (mut client, downstream) = tokio::io::duplex(64 * 1024);
        let (_upstream_client, upstream) = tokio::io::duplex(1024);
        let (mut reader, mut writer) = tokio::io::split(downstream);
        let task = tokio::spawn(async move {
            copy_bidirectional(
                &mut reader,
                &mut writer,
                Box::new(upstream),
                Duration::from_millis(20),
                Duration::from_secs(2),
                Duration::from_secs(2),
                4096,
            )
            .await
        });

        client.write_all(&vec![0_u8; 8192]).await.unwrap();
        let error = task.await.unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
