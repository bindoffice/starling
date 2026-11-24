use super::send_or_discard;
use crate::http::{RequestBodySender, RequestBodyStream};
use crate::server::http2::H2ConnectionFault;
use bytes::Bytes;
use h2::RecvStream;
use std::io;
use std::time::Duration;
use tokio::task::JoinHandle;

pub(in crate::server) struct H2BodyTask {
    stream: RequestBodyStream,
    task: JoinHandle<io::Result<()>>,
}

impl H2BodyTask {
    #[cfg(test)]
    pub(in crate::server) fn spawn(body: RecvStream, timeout: Duration, limit: u64) -> Self {
        Self::spawn_with_connection_fault_and_length(body, timeout, limit, None, None)
    }

    pub(in crate::server) fn spawn_with_connection_fault_and_length(
        body: RecvStream,
        timeout: Duration,
        limit: u64,
        connection_fault: Option<H2ConnectionFault>,
        content_length: Option<u64>,
    ) -> Self {
        let (sender, stream, window) = RequestBodyStream::channel_with_h2_window();
        let task = tokio::spawn(async move {
            let result = pump_h2_body(
                body,
                timeout,
                limit,
                content_length,
                &sender,
                connection_fault.as_ref(),
                window,
            )
            .await;
            if let Err(error) = &result {
                sender
                    .fail(crate::http::clone_request_body_error(error))
                    .await;
            }
            result
        });
        Self { stream, task }
    }

    pub(in crate::server) fn stream(&self) -> RequestBodyStream {
        self.stream.clone()
    }

    pub(in crate::server) async fn close(&self) {
        self.stream.close().await;
    }

    pub(in crate::server) fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    pub(in crate::server) async fn finish(self) -> io::Result<()> {
        self.task
            .await
            .map_err(|error| io::Error::other(error.to_string()))?
    }

    pub(in crate::server) fn detach(self) {
        // Dropping a Tokio JoinHandle detaches the body pump. The receiver has
        // already been closed, so the pump discards remaining DATA while still
        // observing protocol-level events such as request trailers.
        drop(self);
    }
}

async fn pump_h2_body(
    mut body: RecvStream,
    timeout: Duration,
    limit: u64,
    content_length: Option<u64>,
    sender: &RequestBodySender,
    connection_fault: Option<&H2ConnectionFault>,
    mut window: tokio::sync::watch::Receiver<Option<u32>>,
) -> io::Result<()> {
    let mut total = 0_u64;
    let mut active = true;
    loop {
        let data = match wait_for_h2_data(&mut body, timeout, &mut window).await? {
            Some(data) => data,
            None => break,
        };
        let Some(data) = data else {
            break;
        };
        let mut next_data = Some(data.map_err(h2_body_io_error)?);

        // Nginx presents DATA frames received in one read event as one request
        // body chunk. Drain frames that are already buffered without waiting
        // for another network event, while preserving flow-control accounting.
        let mut chunks = Vec::new();
        while let Some(data) = next_data.take() {
            release_h2_body_capacity(&mut body, data.len())?;
            let new_total = total
                .checked_add(data.len() as u64)
                .ok_or_else(crate::http::request_body_too_large_error)?;
            if limit != 0 && new_total > limit {
                send_h2_chunks(sender, chunks, &mut active).await;
                return Err(crate::http::request_body_too_large_error());
            }
            if content_length.is_some_and(|expected| new_total > expected) {
                send_h2_chunks(sender, chunks, &mut active).await;
                return Err(content_length_mismatch_error());
            }
            total = new_total;
            chunks.push(data);
            next_data = match poll_h2_data_now(&mut body).await {
                Some(Some(data)) => Some(data.map_err(h2_body_io_error)?),
                _ => None,
            };
        }
        let end_stream = body.is_end_stream();
        let content_length_mismatch =
            end_stream && content_length.is_some_and(|expected| total != expected);
        if content_length_mismatch {
            return Err(content_length_mismatch_error());
        }
        if end_stream {
            sender.finish();
        }
        send_h2_chunks(sender, chunks, &mut active).await;
    }

    let trailers = if timeout == Duration::ZERO {
        body.trailers().await
    } else {
        match tokio::time::timeout(timeout, body.trailers()).await {
            Ok(trailers) => trailers,
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "request body timed out",
                ));
            }
        }
    };
    if trailers.map_err(h2_body_io_error)?.is_some() {
        if let Some(fault) = connection_fault {
            fault.protocol_error();
        }
        return Err(crate::http::malformed_request_body_error(
            "request trailers are not supported",
        ));
    }
    if content_length.is_some_and(|expected| total != expected) {
        return Err(content_length_mismatch_error());
    }
    sender.finish();
    Ok(())
}

async fn send_h2_chunks(sender: &RequestBodySender, chunks: Vec<Bytes>, active: &mut bool) {
    let data = if chunks.len() == 1 {
        chunks.into_iter().next().expect("one request body chunk")
    } else {
        let length = chunks.iter().map(Bytes::len).sum();
        if length == 0 {
            return;
        }
        let mut combined = Vec::with_capacity(length);
        for chunk in chunks {
            combined.extend_from_slice(&chunk);
        }
        Bytes::from(combined)
    };
    if !data.is_empty() {
        send_or_discard(sender, &data, active).await;
    }
}

async fn wait_for_h2_data(
    body: &mut RecvStream,
    timeout: Duration,
    window: &mut tokio::sync::watch::Receiver<Option<u32>>,
) -> io::Result<Option<Option<Result<Bytes, h2::Error>>>> {
    let next = async {
        let mut window_open = true;
        loop {
            apply_h2_window_target(body, window)?;
            tokio::select! {
                changed = window.changed(), if window_open => {
                    if changed.is_err() {
                        window_open = false;
                    }
                }
                data = body.data() => {
                    return Ok::<Option<Result<Bytes, h2::Error>>, io::Error>(data);
                }
            }
        }
    };
    if timeout == Duration::ZERO {
        return Ok(Some(next.await?));
    }
    match tokio::time::timeout(timeout, next).await {
        Ok(data) => Ok(Some(data?)),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "request body timed out",
        )),
    }
}

fn apply_h2_window_target(
    body: &mut RecvStream,
    window: &tokio::sync::watch::Receiver<Option<u32>>,
) -> io::Result<()> {
    let target = *window.borrow();
    if let Some(target) = target {
        body.set_target_window_size(target)
            .map_err(io::Error::other)?;
    }
    Ok(())
}

async fn poll_h2_data_now(body: &mut RecvStream) -> Option<Option<Result<Bytes, h2::Error>>> {
    std::future::poll_fn(|context| {
        std::task::Poll::Ready(match body.poll_data(context) {
            std::task::Poll::Ready(data) => Some(data),
            std::task::Poll::Pending => None,
        })
    })
    .await
}

fn release_h2_body_capacity(body: &mut RecvStream, length: usize) -> io::Result<()> {
    body.flow_control()
        .release_capacity(length)
        .map_err(io::Error::other)
}

fn content_length_mismatch_error() -> io::Error {
    crate::http::malformed_request_body_error("request body length does not match Content-Length")
}

fn h2_body_io_error(error: h2::Error) -> io::Error {
    if error.is_reset() || error.is_go_away() {
        return io::Error::new(io::ErrorKind::ConnectionAborted, error);
    }
    if error.is_io() {
        return error
            .into_io()
            .unwrap_or_else(|| io::Error::new(io::ErrorKind::ConnectionAborted, "HTTP/2 I/O"));
    }
    io::Error::other(error)
}
