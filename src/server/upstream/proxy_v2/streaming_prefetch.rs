use super::headers::convert_headers;
use super::*;
use bytes::Bytes;
use h2::RecvStream;

pub(in crate::server) enum H2UntilEndPrefetch {
    Complete {
        body: Vec<u8>,
        trailers: Vec<Header>,
    },
    Stream(H2PrefetchedBody),
}

pub(in crate::server) struct H2PrefetchedBody {
    pub prefix: Vec<u8>,
    pub pending: Vec<u8>,
    pub rate: Option<ReadRate>,
}

pub(in crate::server) async fn prefetch_h2_content_length(
    stream: &mut RecvStream,
    length: u64,
    prefix_length: u64,
    timeout: Duration,
    mut rate: Option<ReadRate>,
) -> io::Result<H2PrefetchedBody> {
    if prefix_length > length {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "HTTP/2 prefetch exceeds Content-Length",
        ));
    }
    let capacity = usize::try_from(prefix_length).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP/2 prefetch length does not fit memory",
        )
    })?;
    let mut prefix = Vec::with_capacity(capacity);
    let mut pending = Vec::new();
    while (prefix.len() as u64) < prefix_length {
        let Some(chunk) = next_h2_data(stream, timeout).await? else {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "upstream HTTP/2 response ended before Content-Length",
            ));
        };
        let chunk_len = chunk.len();
        if let Some(rate) = rate.as_mut() {
            rate.account(chunk_len as u64).await;
        }
        let needed = (prefix_length - prefix.len() as u64) as usize;
        if chunk_len > needed {
            prefix.extend_from_slice(&chunk[..needed]);
            pending.extend_from_slice(&chunk[needed..]);
            release_capacity(stream, chunk_len)?;
            break;
        }
        prefix.extend_from_slice(&chunk);
        release_capacity(stream, chunk_len)?;
    }
    Ok(H2PrefetchedBody {
        prefix,
        pending,
        rate,
    })
}

pub(in crate::server) async fn prefetch_h2_until_end(
    stream: &mut RecvStream,
    threshold: u64,
    timeout: Duration,
    mut rate: Option<ReadRate>,
) -> io::Result<H2UntilEndPrefetch> {
    let mut prefix = Vec::new();
    loop {
        if prefix.len() as u64 >= threshold {
            return Ok(H2UntilEndPrefetch::Stream(H2PrefetchedBody {
                prefix,
                pending: Vec::new(),
                rate,
            }));
        }
        let Some(chunk) = next_h2_data(stream, timeout).await? else {
            let trailers = read_h2_trailers(stream, timeout).await?;
            return Ok(H2UntilEndPrefetch::Complete {
                body: prefix,
                trailers,
            });
        };
        let chunk_len = chunk.len();
        if let Some(rate) = rate.as_mut() {
            rate.account(chunk_len as u64).await;
        }
        let needed = (threshold - prefix.len() as u64) as usize;
        if chunk_len > needed {
            prefix.extend_from_slice(&chunk[..needed]);
            let pending = chunk[needed..].to_vec();
            release_capacity(stream, chunk_len)?;
            return Ok(H2UntilEndPrefetch::Stream(H2PrefetchedBody {
                prefix,
                pending,
                rate,
            }));
        }
        prefix.extend_from_slice(&chunk);
        release_capacity(stream, chunk_len)?;
    }
}

pub(super) async fn next_h2_data(
    stream: &mut RecvStream,
    timeout: Duration,
) -> io::Result<Option<Bytes>> {
    let next = if timeout == Duration::ZERO {
        stream.data().await
    } else {
        match tokio::time::timeout(timeout, stream.data()).await {
            Ok(next) => next,
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "upstream HTTP/2 response body timed out",
                ));
            }
        }
    };
    next.map(|chunk| {
        chunk.map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid upstream HTTP/2 response body",
            )
        })
    })
    .transpose()
}

pub(super) fn release_capacity(stream: &mut RecvStream, length: usize) -> io::Result<()> {
    stream.flow_control().release_capacity(length).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "failed to release upstream HTTP/2 flow control capacity",
        )
    })
}

pub(super) async fn read_h2_trailers(
    stream: &mut RecvStream,
    timeout: Duration,
) -> io::Result<Vec<Header>> {
    let trailers = if timeout == Duration::ZERO {
        stream.trailers().await
    } else {
        match tokio::time::timeout(timeout, stream.trailers()).await {
            Ok(trailers) => trailers,
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "upstream HTTP/2 response trailers timed out",
                ));
            }
        }
    };
    let trailers = trailers.map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid upstream HTTP/2 response trailers",
        )
    })?;
    match trailers {
        Some(trailers) => convert_headers(&trailers).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid upstream HTTP/2 response trailers",
            )
        }),
        None => Ok(Vec::new()),
    }
}
