use super::*;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::server) struct BidirectionalCopyStats {
    pub(in crate::server) downstream_to_upstream: u64,
    pub(in crate::server) upstream_to_downstream: u64,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::server) struct BidirectionalCopyTimeouts {
    pub(in crate::server) upstream_write: Duration,
    pub(in crate::server) upstream_read: Duration,
    pub(in crate::server) downstream_write: Duration,
    pub(in crate::server) buffer_size: usize,
}

pub(in crate::server) async fn copy_bidirectional<R, W, S>(
    downstream_reader: &mut R,
    downstream_writer: &mut W,
    upstream: S,
    timeouts: BidirectionalCopyTimeouts,
    stats: &mut BidirectionalCopyStats,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut upstream_reader, mut upstream_writer) = tokio::io::split(upstream);
    let uploaded = Arc::new(AtomicU64::new(0));
    let downloaded = Arc::new(AtomicU64::new(0));
    let upload = copy_direction(
        downstream_reader,
        &mut upstream_writer,
        &uploaded,
        None,
        timeouts.upstream_write,
        timeouts.buffer_size,
    );
    let download = copy_direction(
        &mut upstream_reader,
        downstream_writer,
        &downloaded,
        Some(timeouts.upstream_read),
        timeouts.downstream_write,
        timeouts.buffer_size,
    );
    tokio::pin!(upload);
    tokio::pin!(download);
    let mut upload_done = false;
    let mut download_done = false;
    let result = loop {
        if upload_done && download_done {
            break Ok(());
        }
        tokio::select! {
            result = &mut upload, if !upload_done => {
                upload_done = true;
                if let Err(error) = result {
                    break Err(error);
                }
            }
            result = &mut download, if !download_done => {
                download_done = true;
                if let Err(error) = result {
                    break Err(error);
                }
            }
        }
    };
    *stats = BidirectionalCopyStats {
        downstream_to_upstream: uploaded.load(Ordering::Relaxed),
        upstream_to_downstream: downloaded.load(Ordering::Relaxed),
    };
    result
}

async fn copy_direction<R, W>(
    reader: &mut R,
    writer: &mut W,
    copied: &AtomicU64,
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
        copied.fetch_add(read as u64, Ordering::Relaxed);
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
            let mut stats = BidirectionalCopyStats::default();
            copy_bidirectional(
                &mut reader,
                &mut writer,
                Box::new(upstream),
                BidirectionalCopyTimeouts {
                    upstream_write: Duration::from_secs(2),
                    upstream_read: Duration::from_secs(2),
                    downstream_write: Duration::from_secs(2),
                    buffer_size: 4096,
                },
                &mut stats,
            )
            .await
            .map(|()| stats)
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

        let stats = task.await.unwrap().unwrap();
        assert_eq!(
            stats,
            BidirectionalCopyStats {
                downstream_to_upstream: 18,
                upstream_to_downstream: 18,
            }
        );
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
                BidirectionalCopyTimeouts {
                    upstream_write: Duration::from_secs(2),
                    upstream_read: Duration::from_secs(2),
                    downstream_write: Duration::from_secs(2),
                    buffer_size: 4096,
                },
                &mut BidirectionalCopyStats::default(),
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
                BidirectionalCopyTimeouts {
                    upstream_write: Duration::from_millis(20),
                    upstream_read: Duration::from_secs(2),
                    downstream_write: Duration::from_secs(2),
                    buffer_size: 4096,
                },
                &mut BidirectionalCopyStats::default(),
            )
            .await
        });

        client.write_all(&vec![0_u8; 8192]).await.unwrap();
        let error = task.await.unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
