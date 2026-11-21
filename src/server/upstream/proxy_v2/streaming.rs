use super::connection::ProxyV2DriverGuard;
use super::streaming_prefetch::{
    H2PrefetchedBody, next_h2_data, read_h2_trailers, release_capacity,
};
use super::*;
use tokio::sync::mpsc;

pub(super) fn spawn_h2_body_stream(
    mut stream: h2::RecvStream,
    length: Option<u64>,
    read_timeout: Duration,
    limit_rate: u64,
    driver: ProxyV2DriverGuard,
) -> ResponseBody {
    let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
    let sink = StreamSender::Channel(sender.clone());
    tokio::spawn(run_channel_producer(sender, async move {
        let _driver = driver;
        let mut rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
        pump_h2_body(&mut stream, read_timeout, rate.as_mut(), &sink).await
    }));
    ResponseBody::Stream(ResponseStream::new(receiver, length, true))
}

pub(super) fn spawn_prefetched_h2_body_stream(
    mut stream: h2::RecvStream,
    prefetched: H2PrefetchedBody,
    length: Option<u64>,
    read_timeout: Duration,
    driver: ProxyV2DriverGuard,
    buffer_options: UpstreamBodyBufferOptions,
) -> ResponseBody {
    spawn_buffered_stream(buffer_options, length, true, move |sender| async move {
        let _driver = driver;
        let H2PrefetchedBody {
            prefix,
            pending,
            mut rate,
        } = prefetched;
        pump_h2_body_with_prefix(
            &mut stream,
            prefix,
            pending,
            read_timeout,
            rate.as_mut(),
            &sender,
        )
        .await
    })
}

async fn pump_h2_body(
    stream: &mut h2::RecvStream,
    read_timeout: Duration,
    rate: Option<&mut ReadRate>,
    sender: &StreamSender,
) -> io::Result<()> {
    pump_h2_body_with_prefix(stream, Vec::new(), Vec::new(), read_timeout, rate, sender).await
}

async fn pump_h2_body_with_prefix(
    stream: &mut h2::RecvStream,
    prefix: Vec<u8>,
    pending: Vec<u8>,
    read_timeout: Duration,
    mut rate: Option<&mut ReadRate>,
    sender: &StreamSender,
) -> io::Result<()> {
    send_data(sender, &prefix, None).await?;
    send_data(sender, &pending, None).await?;
    loop {
        let Some(chunk) = next_h2_data(stream, read_timeout).await? else {
            break;
        };
        let length = chunk.len();
        send_data(sender, &chunk, rate.as_deref_mut()).await?;
        release_capacity(stream, length)?;
    }
    let trailers = read_h2_trailers(stream, read_timeout).await?;
    send_trailers(sender, trailers).await?;
    Ok(())
}
