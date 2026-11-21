use super::super::streaming::{
    STREAM_READ_SIZE, StreamSender, return_stream_connection, send_data, spawn_buffered_stream,
    with_read_timeout,
};
use super::super::*;
use tokio::io::AsyncReadExt;

pub(in crate::server) async fn prefetch_content_length<R>(
    reader: &mut R,
    length: u64,
    prefix_length: u64,
    timeout: Duration,
    mut rate: Option<ReadRate>,
) -> io::Result<(Vec<u8>, Option<ReadRate>)>
where
    R: AsyncReadExt + Unpin,
{
    if prefix_length > length {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "upstream prefetch exceeds Content-Length",
        ));
    }
    let capacity = usize::try_from(prefix_length).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "upstream prefetch length does not fit memory",
        )
    })?;
    let mut prefix = Vec::with_capacity(capacity);
    while (prefix.len() as u64) < prefix_length {
        let remaining = prefix_length - prefix.len() as u64;
        let wanted = remaining.min(STREAM_READ_SIZE as u64) as usize;
        let start = prefix.len();
        prefix.resize(start + wanted, 0);
        let read = match with_read_timeout(
            timeout,
            reader.read(&mut prefix[start..]),
            "upstream response body timed out",
        )
        .await
        {
            Ok(read) => read,
            Err(error) => {
                prefix.truncate(start);
                return Err(error);
            }
        };
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "upstream response ended before Content-Length",
            ));
        }
        prefix.truncate(start + read);
        if let Some(rate) = rate.as_mut() {
            rate.account(read as u64).await;
        }
    }
    Ok((prefix, rate))
}

pub(in crate::server) fn spawn_prefetched_content_length_stream(
    connection: UpstreamConnection,
    prefix: Vec<u8>,
    length: u64,
    timeout: Duration,
    rate: Option<ReadRate>,
    buffer_options: UpstreamBodyBufferOptions,
    reuse: Option<tokio::sync::oneshot::Sender<UpstreamConnection>>,
) -> ResponseBody {
    let UpstreamConnection {
        reader,
        key,
        created,
        idle_since,
        requests,
    } = connection;
    spawn_buffered_stream(
        buffer_options,
        Some(length),
        false,
        move |sender| async move {
            let mut reader = reader;
            let result =
                pump_prefetched_body(&mut reader, prefix, length, timeout, rate, &sender).await;
            if result.is_ok() {
                return_stream_connection(reuse, reader, key, created, idle_since, requests);
            }
            result
        },
    )
}

async fn pump_prefetched_body<R>(
    reader: &mut R,
    prefix: Vec<u8>,
    length: u64,
    timeout: Duration,
    mut rate: Option<ReadRate>,
    sender: &StreamSender,
) -> io::Result<()>
where
    R: AsyncReadExt + Unpin,
{
    let prefix_length = prefix.len() as u64;
    if prefix_length > length {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "upstream prefetch exceeds Content-Length",
        ));
    }
    send_data(sender, &prefix, None).await?;

    let mut remaining = length - prefix_length;
    let mut buffer = vec![0_u8; STREAM_READ_SIZE];
    while remaining > 0 {
        let wanted = remaining.min(buffer.len() as u64) as usize;
        let read = with_read_timeout(
            timeout,
            reader.read(&mut buffer[..wanted]),
            "upstream response body timed out",
        )
        .await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "upstream response ended before Content-Length",
            ));
        }
        send_data(sender, &buffer[..read], rate.as_mut()).await?;
        remaining -= read as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn prefetch_times_out_before_the_response_is_committed() {
        let (mut peer, mut reader) = tokio::io::duplex(1024);
        peer.write_all(&[b'x'; 100]).await.unwrap();
        let started = Instant::now();
        let error = match prefetch_content_length(
            &mut reader,
            12 * 1024,
            8 * 1024,
            Duration::from_millis(50),
            None,
        )
        .await
        {
            Ok(_) => panic!("prefetch should time out"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn prefetch_rejects_truncation_before_the_busy_threshold() {
        let (mut peer, mut reader) = tokio::io::duplex(1024);
        peer.write_all(&[b'x'; 100]).await.unwrap();
        peer.shutdown().await.unwrap();
        let error = match prefetch_content_length(
            &mut reader,
            12 * 1024,
            8 * 1024,
            Duration::from_secs(1),
            None,
        )
        .await
        {
            Ok(_) => panic!("prefetch should reject a truncated body"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }
}
