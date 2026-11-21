use super::*;
use tokio::sync::mpsc;

pub(in crate::server) struct FastCgiResponseReadOptions<'a> {
    pub(in crate::server) catch_stderr: &'a [String],
    pub(in crate::server) limit_rate: u64,
    pub(in crate::server) max_header_bytes: usize,
    pub(in crate::server) buffer_policy: UpstreamBodyBufferPolicy,
    pub(in crate::server) streaming: Option<&'a GatewayStreamingPolicy<'a>>,
    pub(in crate::server) context: &'a EffectiveConfig,
    pub(in crate::server) read_timeout: Duration,
}

pub(in crate::server) async fn read_fastcgi_response_streaming<R>(
    mut reader: R,
    options: FastCgiResponseReadOptions<'_>,
) -> io::Result<GatewayResponseRead<R>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let FastCgiResponseReadOptions {
        catch_stderr,
        limit_rate,
        max_header_bytes,
        buffer_policy,
        streaming,
        context,
        read_timeout,
    } = options;
    let mut decoder = CgiHeadDecoder::new(max_header_bytes);
    let mut read_rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
    loop {
        let record = read_fastcgi_record_with_timeout(&mut reader, read_timeout)
            .await
            .map_err(fastcgi_timeout_error)?;
        match record.record_type {
            FCGI_STDERR => reject_matching_stderr(&record.content, catch_stderr)?,
            FCGI_STDOUT => {
                if let Some(rate) = read_rate.as_mut() {
                    rate.account(record.content.len() as u64).await;
                }
                let Some(mut start) = decoder.push(&record.content)? else {
                    continue;
                };
                truncate_prefix_to_content_length(
                    &mut start.body_prefix,
                    start.head.content_length(),
                );
                let handling = streaming
                    .map(|policy| {
                        gateway_body_handling(
                            context,
                            policy,
                            start.head.status(),
                            start.head.headers(),
                        )
                    })
                    .unwrap_or(GatewayBodyHandling::CompleteBuffer);
                match handling {
                    GatewayBodyHandling::DirectStream => {
                        let body = spawn_fastcgi_body_stream(
                            reader,
                            start.body_prefix,
                            start.head.content_length(),
                            catch_stderr,
                            read_timeout,
                            limit_rate,
                        );
                        return Ok(GatewayResponseRead::Streamed(
                            start.head.into_response(body, None)?,
                        ));
                    }
                    GatewayBodyHandling::BufferedStream { threshold } => {
                        return read_buffered_fastcgi_response(
                            reader,
                            start,
                            threshold,
                            catch_stderr,
                            limit_rate,
                            read_timeout,
                            buffer_policy,
                        )
                        .await;
                    }
                    GatewayBodyHandling::CompleteBuffer => {
                        let (response, protocol_complete) = finish_fastcgi_response(
                            &mut reader,
                            start,
                            catch_stderr,
                            limit_rate,
                            read_timeout,
                            buffer_policy,
                        )
                        .await?;
                        return Ok(GatewayResponseRead::Buffered {
                            response,
                            reader,
                            protocol_complete,
                        });
                    }
                }
            }
            FCGI_END_REQUEST => {
                let start = decoder.finish()?;
                return Ok(GatewayResponseRead::Buffered {
                    response: start
                        .head
                        .into_response(ResponseBody::Bytes(start.body_prefix), None)?,
                    reader,
                    protocol_complete: true,
                });
            }
            _ => {}
        }
    }
}

async fn read_buffered_fastcgi_response<R>(
    mut reader: R,
    start: CgiHeadStart,
    threshold: usize,
    catch_stderr: &[String],
    limit_rate: u64,
    read_timeout: Duration,
    buffer_policy: UpstreamBodyBufferPolicy,
) -> io::Result<GatewayResponseRead<R>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let length = start.head.content_length();
    let buffered_options = buffer_policy.options_for_headers(start.head.headers());
    let mut prefix = start.body_prefix;
    if length.is_some_and(|length| length > threshold as u64) || prefix.len() > threshold {
        let body = spawn_buffered_fastcgi_body_stream(
            reader,
            prefix,
            length,
            catch_stderr,
            read_timeout,
            limit_rate,
            buffered_options,
        );
        return Ok(GatewayResponseRead::Streamed(
            start.head.into_response(body, None)?,
        ));
    }

    let mut rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
    if length.is_some_and(|length| prefix.len() as u64 == length) {
        let response = start
            .head
            .into_response(ResponseBody::Bytes(prefix), None)?;
        return Ok(GatewayResponseRead::Buffered {
            response,
            reader,
            protocol_complete: false,
        });
    }
    loop {
        if prefix.len() > threshold {
            let body = spawn_buffered_fastcgi_body_stream(
                reader,
                prefix,
                length,
                catch_stderr,
                read_timeout,
                limit_rate,
                buffered_options,
            );
            return Ok(GatewayResponseRead::Streamed(
                start.head.into_response(body, None)?,
            ));
        }
        let record = read_fastcgi_record_with_timeout(&mut reader, read_timeout)
            .await
            .map_err(fastcgi_timeout_error)?;
        match record.record_type {
            FCGI_STDERR => reject_matching_stderr(&record.content, catch_stderr)?,
            FCGI_STDOUT => {
                if let Some(rate) = rate.as_mut() {
                    rate.account(record.content.len() as u64).await;
                }
                let content = match length {
                    Some(length) => {
                        let remaining = length.saturating_sub(prefix.len() as u64) as usize;
                        &record.content[..record.content.len().min(remaining)]
                    }
                    None => record.content.as_slice(),
                };
                prefix.extend_from_slice(content);
                if length.is_some_and(|length| prefix.len() as u64 == length) {
                    let response = start
                        .head
                        .into_response(ResponseBody::Bytes(prefix), None)?;
                    return Ok(GatewayResponseRead::Buffered {
                        response,
                        reader,
                        protocol_complete: false,
                    });
                }
            }
            FCGI_END_REQUEST => {
                if length.is_some_and(|length| prefix.len() as u64 != length) {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "FastCGI response ended before Content-Length",
                    ));
                }
                let response = start
                    .head
                    .into_response(ResponseBody::Bytes(prefix), None)?;
                return Ok(GatewayResponseRead::Buffered {
                    response,
                    reader,
                    protocol_complete: true,
                });
            }
            _ => {}
        }
    }
}

async fn finish_fastcgi_response<R>(
    reader: &mut R,
    start: CgiHeadStart,
    catch_stderr: &[String],
    limit_rate: u64,
    read_timeout: Duration,
    buffer_policy: UpstreamBodyBufferPolicy,
) -> io::Result<(Response, bool)>
where
    R: AsyncRead + Unpin,
{
    let expected_length = start.head.content_length();
    let mut body_length = start.body_prefix.len() as u64;
    let mut body = BodyBuffer::new(buffer_policy.options_for_headers(start.head.headers()));
    body.append(&start.body_prefix).await?;
    if expected_length.is_some_and(|length| body_length == length) {
        let buffered = body.finish().await?;
        return start
            .head
            .into_response(buffered.body, buffered.temp_file)
            .map(|response| (response, false));
    }
    let mut read_rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
    loop {
        let record = read_fastcgi_record_with_timeout(reader, read_timeout)
            .await
            .map_err(fastcgi_timeout_error)?;
        match record.record_type {
            FCGI_STDERR => reject_matching_stderr(&record.content, catch_stderr)?,
            FCGI_STDOUT => {
                if let Some(rate) = read_rate.as_mut() {
                    rate.account(record.content.len() as u64).await;
                }
                let content = match expected_length {
                    Some(length) => {
                        let remaining = length.saturating_sub(body_length) as usize;
                        &record.content[..record.content.len().min(remaining)]
                    }
                    None => record.content.as_slice(),
                };
                body_length += content.len() as u64;
                body.append(content).await?;
                if expected_length.is_some_and(|length| body_length == length) {
                    let buffered = body.finish().await?;
                    return start
                        .head
                        .into_response(buffered.body, buffered.temp_file)
                        .map(|response| (response, false));
                }
            }
            FCGI_END_REQUEST => {
                if expected_length.is_some_and(|length| body_length != length) {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "FastCGI response ended before Content-Length",
                    ));
                }
                let buffered = body.finish().await?;
                return start
                    .head
                    .into_response(buffered.body, buffered.temp_file)
                    .map(|response| (response, true));
            }
            _ => {}
        }
    }
}

fn spawn_fastcgi_body_stream<R>(
    mut reader: R,
    prefix: Vec<u8>,
    length: Option<u64>,
    catch_stderr: &[String],
    read_timeout: Duration,
    limit_rate: u64,
) -> ResponseBody
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let catch_stderr = catch_stderr.to_vec();
    let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
    let sink = StreamSender::Channel(sender.clone());
    tokio::spawn(run_channel_producer(sender, async move {
        let mut rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
        pump_fastcgi_body(
            &mut reader,
            &prefix,
            length,
            &catch_stderr,
            read_timeout,
            rate.as_mut(),
            &sink,
        )
        .await
    }));
    ResponseBody::Stream(ResponseStream::new(receiver, length, false))
}

fn spawn_buffered_fastcgi_body_stream<R>(
    mut reader: R,
    prefix: Vec<u8>,
    length: Option<u64>,
    catch_stderr: &[String],
    read_timeout: Duration,
    limit_rate: u64,
    buffer_options: UpstreamBodyBufferOptions,
) -> ResponseBody
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let catch_stderr = catch_stderr.to_vec();
    spawn_buffered_stream(buffer_options, length, false, move |sender| async move {
        let mut rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
        pump_fastcgi_body(
            &mut reader,
            &prefix,
            length,
            &catch_stderr,
            read_timeout,
            rate.as_mut(),
            &sender,
        )
        .await
    })
}

async fn pump_fastcgi_body<R>(
    reader: &mut R,
    prefix: &[u8],
    length: Option<u64>,
    catch_stderr: &[String],
    read_timeout: Duration,
    mut rate: Option<&mut ReadRate>,
    sender: &StreamSender,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let prefix_length = prefix.len() as u64;
    if length.is_some_and(|length| prefix_length > length) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "FastCGI response body exceeds Content-Length",
        ));
    }
    // Header parsing already applied the read rate to the records containing this prefix.
    send_data(sender, prefix, None).await?;
    let mut remaining = length.map(|length| length - prefix_length);
    if remaining == Some(0) {
        return Ok(());
    }

    loop {
        let record = read_fastcgi_record_with_timeout(reader, read_timeout)
            .await
            .map_err(fastcgi_timeout_error)?;
        match record.record_type {
            FCGI_STDERR => reject_matching_stderr(&record.content, catch_stderr)?,
            FCGI_STDOUT => {
                let data = match remaining.as_mut() {
                    Some(remaining) => {
                        let size = (*remaining).min(record.content.len() as u64) as usize;
                        *remaining -= size as u64;
                        let data = &record.content[..size];
                        send_data(sender, data, rate.as_deref_mut()).await?;
                        if *remaining == 0 {
                            return Ok(());
                        }
                        continue;
                    }
                    None => record.content.as_slice(),
                };
                send_data(sender, data, rate.as_deref_mut()).await?;
            }
            FCGI_END_REQUEST => {
                if remaining.is_some_and(|remaining| remaining != 0) {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "FastCGI response ended before Content-Length",
                    ));
                }
                return Ok(());
            }
            _ => {}
        }
    }
}

fn reject_matching_stderr(content: &[u8], catch_stderr: &[String]) -> io::Result<()> {
    if content.is_empty()
        || !catch_stderr
            .iter()
            .any(|pattern| String::from_utf8_lossy(content).contains(pattern.as_str()))
    {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "FastCGI stderr matched fastcgi_catch_stderr",
    ))
}

fn fastcgi_timeout_error(error: io::Error) -> io::Error {
    if error.kind() == io::ErrorKind::TimedOut {
        io::Error::new(io::ErrorKind::TimedOut, "FastCGI response timed out")
    } else {
        error
    }
}
