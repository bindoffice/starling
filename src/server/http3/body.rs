use super::*;
use crate::http::{RequestBodySender, RequestBodyStream};
use bytes::Buf;
use h3::error::Code;
use std::io;
use std::time::Duration;
use tokio::task::JoinHandle;

pub(super) struct H3BodyTask {
    stream: RequestBodyStream,
    task: JoinHandle<io::Result<()>>,
}

impl H3BodyTask {
    pub(super) fn spawn(
        mut body: H3RecvStream,
        timeout: Duration,
        limit: u64,
        content_length: Option<u64>,
    ) -> Self {
        let (sender, stream) = RequestBodyStream::channel();
        let task = tokio::spawn(async move {
            let result = pump_h3_body(&mut body, timeout, limit, content_length, &sender).await;
            if let Err(error) = &result {
                body.stop_sending(Code::H3_REQUEST_CANCELLED);
                sender
                    .fail(crate::http::clone_request_body_error(error))
                    .await;
            }
            result
        });
        Self { stream, task }
    }

    pub(super) fn stream(&self) -> RequestBodyStream {
        self.stream.clone()
    }

    pub(super) async fn close(&self) {
        self.stream.close().await;
    }

    pub(super) fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    pub(super) async fn finish(self) -> io::Result<()> {
        self.task
            .await
            .map_err(|error| io::Error::other(error.to_string()))?
    }

    pub(super) fn detach(self) {
        drop(self);
    }
}

async fn pump_h3_body(
    body: &mut H3RecvStream,
    timeout: Duration,
    limit: u64,
    content_length: Option<u64>,
    sender: &RequestBodySender,
) -> io::Result<()> {
    let mut total = 0_u64;
    let mut active = true;
    loop {
        let data = timeout_stream(body.recv_data(), timeout, "request body timed out").await?;
        let Some(mut data) = data else {
            break;
        };
        let length = data.remaining();
        if length == 0 {
            continue;
        }
        total = total
            .checked_add(length as u64)
            .ok_or_else(crate::http::request_body_too_large_error)?;
        if limit != 0 && total > limit {
            return Err(crate::http::request_body_too_large_error());
        }
        if content_length.is_some_and(|expected| total > expected) {
            return Err(content_length_mismatch_error());
        }
        let bytes = data.copy_to_bytes(length);
        if active && !sender.send(bytes).await {
            active = false;
        }
    }

    let trailers =
        timeout_stream(body.recv_trailers(), timeout, "request trailers timed out").await?;
    if trailers.is_some() {
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

async fn timeout_stream<F, T>(future: F, timeout: Duration, message: &'static str) -> io::Result<T>
where
    F: std::future::Future<Output = Result<T, h3::error::StreamError>>,
{
    let result = if timeout.is_zero() {
        future.await
    } else {
        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, message))?
    };
    result.map_err(|error| io::Error::other(format!("{message}: {error}")))
}

fn content_length_mismatch_error() -> io::Error {
    crate::http::malformed_request_body_error("request body length does not match Content-Length")
}
