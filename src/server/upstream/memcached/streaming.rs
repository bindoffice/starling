use super::*;
use tokio::sync::mpsc;

pub(in crate::server) enum MemcachedExchange {
    Buffered {
        response: MemcachedResponse,
        connection: Box<UpstreamConnection>,
    },
    Streamed {
        body: ResponseBody,
        gzip: bool,
    },
}

pub(in crate::server) async fn memcached_exchange_connection_streaming(
    mut connection: UpstreamConnection,
    key: &[u8],
    request: &Request,
    context: &EffectiveConfig,
    keepalive: Option<UpstreamKeepaliveContext>,
    worker: usize,
    pool: Arc<UpstreamKeepalivePool>,
) -> Result<MemcachedExchange, ProxyFailure> {
    connection.requests = connection.requests.saturating_add(1);
    write_memcached_request(&mut connection, key, context).await?;
    let head = {
        let mut reader = TimedReader::new(&mut connection.reader, context.memcached_read_timeout);
        read_memcached_head(
            &mut reader,
            key,
            context.memcached_gzip_flag,
            context.memcached_buffer_size,
        )
        .await
        .map_err(proxy_failure_from_io)?
    };

    match head {
        MemcachedHead::NotFound => Ok(MemcachedExchange::Buffered {
            response: MemcachedResponse::NotFound,
            connection: Box::new(connection),
        }),
        MemcachedHead::Value { length, gzip }
            if can_stream_memcached_response(context, request) =>
        {
            let body = spawn_memcached_body_stream(
                connection,
                length,
                context.memcached_read_timeout,
                keepalive.map(|keepalive| (worker, pool, keepalive)),
            );
            Ok(MemcachedExchange::Streamed { body, gzip })
        }
        MemcachedHead::Value { length, gzip } => {
            let body = {
                let mut reader =
                    TimedReader::new(&mut connection.reader, context.memcached_read_timeout);
                read_memcached_body(&mut reader, length)
                    .await
                    .map_err(proxy_failure_from_io)?
            };
            let trailer = {
                let mut reader =
                    TimedReader::new(&mut connection.reader, context.memcached_read_timeout);
                read_memcached_trailer(&mut reader).await
            };
            trailer.map_err(proxy_failure_from_io)?;
            Ok(MemcachedExchange::Buffered {
                response: MemcachedResponse::Value { body, gzip },
                connection: Box::new(connection),
            })
        }
    }
}

async fn write_memcached_request(
    connection: &mut UpstreamConnection,
    key: &[u8],
    context: &EffectiveConfig,
) -> Result<(), ProxyFailure> {
    let request = build_memcached_request(key);
    let write = async {
        let mut writer =
            TimedWriter::new(connection.reader.get_mut(), context.memcached_send_timeout);
        writer.write_all(&request).await?;
        writer.flush().await
    };
    write.await.map_err(proxy_failure_from_io)
}

fn can_stream_memcached_response(context: &EffectiveConfig, request: &Request) -> bool {
    request.method == "GET"
        && request.header("range").is_none()
        && !response_filters_require_buffering(context)
}

fn spawn_memcached_body_stream(
    mut connection: UpstreamConnection,
    length: u64,
    read_timeout: Duration,
    keepalive: Option<(usize, Arc<UpstreamKeepalivePool>, UpstreamKeepaliveContext)>,
) -> ResponseBody {
    let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
    tokio::spawn(async move {
        let result = tokio::select! {
            biased;
            _ = sender.closed() => return,
            result = async {
                let mut reader = TimedReader::new(&mut connection.reader, read_timeout);
                pump_memcached_body(&mut reader, length, &sender).await
            } => result,
        };
        match result {
            Ok(()) => {
                if connection.reader.buffer().is_empty()
                    && let Some((worker, pool, keepalive)) = keepalive
                {
                    pool.put_for(worker, &keepalive.name, &keepalive.config, connection);
                }
            }
            Err(err) => {
                let _ = sender.send(Err(err)).await;
            }
        }
    });
    ResponseBody::Stream(ResponseStream::new(receiver, Some(length), false))
}

async fn pump_memcached_body<R>(
    reader: &mut R,
    length: u64,
    sender: &mpsc::Sender<io::Result<ResponseStreamItem>>,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let sink = StreamSender::Channel(sender.clone());
    let mut remaining = length;
    let mut buffer = vec![0_u8; STREAM_READ_SIZE];
    while remaining > 0 {
        let size = remaining.min(buffer.len() as u64) as usize;
        let read = reader.read(&mut buffer[..size]).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "memcached response ended before Value length",
            ));
        }
        send_data(&sink, &buffer[..read], None).await?;
        remaining -= read as u64;
    }
    read_memcached_trailer(reader).await
}
