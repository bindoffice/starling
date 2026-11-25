use super::*;
use crate::http::RequestHead;
use tokio::io::ReadHalf;

use super::super::body_stream::Http1BodyTask;

pub(super) fn spawn_http1_body<S>(
    reader: &mut Option<BufReader<ReadHalf<S>>>,
    head: &RequestHead,
    invalid_host: bool,
    body_timeout: Duration,
) -> Option<Http1BodyTask<S>>
where
    S: AsyncRead + Send + 'static,
{
    if invalid_host || !(head.chunked || head.content_length.is_some_and(|length| length > 0)) {
        return None;
    }
    let reader = reader
        .take()
        .expect("HTTP/1 reader available for request body");
    // Location selection supplies the real client_max_body_size.
    Http1BodyTask::spawn(reader, head, body_timeout, 0)
}

pub(super) async fn close_http1_body<S>(task: &mut Option<Http1BodyTask<S>>)
where
    S: AsyncRead + Send + 'static,
{
    if let Some(task) = task.take() {
        task.close().await;
        task.abort();
    }
}
