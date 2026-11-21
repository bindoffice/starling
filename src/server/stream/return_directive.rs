use super::*;

const STREAM_RETURN_TIMEOUT: Duration = Duration::from_secs(5);

pub(in crate::server) async fn write_stream_return<S>(
    inbound: &mut S,
    template: &str,
    log: &mut StreamLogContext,
) where
    S: AsyncWrite + Unpin,
{
    let text = eval_stream_log_variables(template, log);
    log.status = 200;
    if text.is_empty() {
        return;
    }

    let result = tokio::time::timeout(STREAM_RETURN_TIMEOUT, async {
        inbound.write_all(text.as_bytes()).await?;
        inbound.flush().await
    })
    .await;

    match result {
        Ok(Ok(())) => {
            log.bytes_sent = text.len() as u64;
        }
        Ok(Err(err)) => {
            log.status = 500;
            log.logger
                .log(LogLevel::Info, format!("stream return write failed: {err}"));
        }
        Err(_) => {
            // Nginx finalizes stream return sessions with OK on write timeout.
            log.status = 200;
            log.logger
                .log(LogLevel::Info, "stream return write timed out");
        }
    }
}
