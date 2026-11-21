use super::super::streaming::{
    STREAM_READ_SIZE, StreamSender, send_data, spawn_buffered_stream, with_read_timeout,
};
use super::super::*;
use tokio::io::AsyncReadExt;

pub(in crate::server) enum UntilEofPrefetch {
    Complete(Vec<u8>),
    Stream {
        prefix: Vec<u8>,
        rate: Option<ReadRate>,
    },
}

pub(in crate::server) async fn prefetch_until_eof<R>(
    reader: &mut R,
    threshold: u64,
    timeout: Duration,
    mut rate: Option<ReadRate>,
) -> io::Result<UntilEofPrefetch>
where
    R: AsyncReadExt + Unpin,
{
    let mut prefix = Vec::new();
    loop {
        if prefix.len() as u64 > threshold {
            return Ok(UntilEofPrefetch::Stream { prefix, rate });
        }
        let wanted = threshold
            .saturating_sub(prefix.len() as u64)
            .saturating_add(1)
            .min(STREAM_READ_SIZE as u64) as usize;
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
            prefix.truncate(start);
            return Ok(UntilEofPrefetch::Complete(prefix));
        }
        prefix.truncate(start + read);
        if let Some(rate) = rate.as_mut() {
            rate.account(read as u64).await;
        }
    }
}

pub(in crate::server) fn spawn_prefetched_until_eof_stream(
    connection: UpstreamConnection,
    prefix: Vec<u8>,
    timeout: Duration,
    rate: Option<ReadRate>,
    buffer_options: UpstreamBodyBufferOptions,
) -> ResponseBody {
    spawn_buffered_stream(buffer_options, None, false, move |sender| async move {
        let mut reader = connection.reader;
        pump_prefetched_until_eof(&mut reader, prefix, timeout, rate, &sender).await
    })
}

async fn pump_prefetched_until_eof<R>(
    reader: &mut R,
    prefix: Vec<u8>,
    timeout: Duration,
    mut rate: Option<ReadRate>,
    sender: &StreamSender,
) -> io::Result<()>
where
    R: AsyncReadExt + Unpin,
{
    send_data(sender, &prefix, None).await?;
    let mut buffer = vec![0_u8; STREAM_READ_SIZE];
    loop {
        let read = with_read_timeout(
            timeout,
            reader.read(&mut buffer),
            "upstream response body timed out",
        )
        .await?;
        if read == 0 {
            return Ok(());
        }
        send_data(sender, &buffer[..read], rate.as_mut()).await?;
    }
}
