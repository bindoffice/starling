use super::*;

pub(in crate::server) fn encode_null_separated_params(parameters: &[(String, String)]) -> Vec<u8> {
    let mut encoded = Vec::new();
    for (name, value) in parameters {
        encoded.extend_from_slice(name.as_bytes());
        encoded.push(0);
        encoded.extend_from_slice(value.as_bytes());
        encoded.push(0);
    }
    encoded
}

#[cfg(test)]
pub(in crate::server) async fn read_cgi_stream_with_policy<R: AsyncRead + Unpin>(
    reader: &mut R,
    limit_rate: u64,
    max_header_bytes: usize,
    buffer_policy: UpstreamBodyBufferPolicy,
) -> io::Result<Response> {
    let start = read_cgi_head(reader, max_header_bytes, Duration::ZERO).await?;
    finish_cgi_response(reader, start, limit_rate, buffer_policy, Duration::ZERO).await
}

pub(in crate::server) async fn read_cgi_head<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_header_bytes: usize,
    read_timeout: Duration,
) -> io::Result<CgiHeadStart> {
    let mut reader = TimedReader::new(reader, read_timeout);
    let mut decoder = CgiHeadDecoder::new(max_header_bytes);
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return decoder.finish();
        }
        if let Some(start) = decoder.push(&buffer[..read])? {
            return Ok(start);
        }
    }
}

pub(in crate::server) async fn finish_cgi_response<R: AsyncRead + Unpin>(
    reader: &mut R,
    start: CgiHeadStart,
    limit_rate: u64,
    buffer_policy: UpstreamBodyBufferPolicy,
    read_timeout: Duration,
) -> io::Result<Response> {
    let mut reader = TimedReader::new(reader, read_timeout);
    let expected_length = start.head.content_length();
    let mut body = BodyBuffer::new(buffer_policy.options_for_headers(start.head.headers()));
    let mut prefix = start.body_prefix.as_slice();
    if let Some(length) = expected_length
        && length < prefix.len() as u64
    {
        prefix = &prefix[..length as usize];
    }
    body.append(prefix).await?;
    let mut read_rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
    if let Some(rate) = read_rate.as_mut() {
        rate.account(prefix.len() as u64).await;
    }
    let mut body_length = prefix.len() as u64;
    if let Some(length) = expected_length {
        let mut buffer = [0_u8; 16 * 1024];
        while body_length < length {
            let wanted = (length - body_length).min(buffer.len() as u64) as usize;
            let read = reader.read(&mut buffer[..wanted]).await?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "CGI response ended before Content-Length",
                ));
            }
            if let Some(rate) = read_rate.as_mut() {
                rate.account(read as u64).await;
            }
            body.append(&buffer[..read]).await?;
            body_length += read as u64;
        }
    } else {
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            if let Some(rate) = read_rate.as_mut() {
                rate.account(read as u64).await;
            }
            body.append(&buffer[..read]).await?;
        }
    }
    let buffered = body.finish().await?;
    start.head.into_response(buffered.body, buffered.temp_file)
}
