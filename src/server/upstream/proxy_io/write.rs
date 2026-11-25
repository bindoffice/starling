use super::*;

#[cfg(test)]
pub(in crate::server) async fn write_proxy_request<S>(
    stream: &mut S,
    outgoing: &ProxyRequestMessage,
    version: &str,
) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    write_proxy_request_timed(stream, outgoing, version, None).await
}

pub(super) async fn write_proxy_request_timed<S>(
    stream: &mut S,
    outgoing: &ProxyRequestMessage,
    version: &str,
    timeout: Option<Duration>,
) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut head = format!("{} {} HTTP/{version}\r\n", outgoing.method, outgoing.path);
    let cookies: Vec<&str> = outgoing
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("cookie"))
        .map(|(_, value)| value.as_str())
        .collect();
    let mut wrote_cookies = false;
    for (name, value) in &outgoing.headers {
        if name.eq_ignore_ascii_case("cookie") {
            if wrote_cookies {
                continue;
            }
            wrote_cookies = true;
            head.push_str(name);
            head.push_str(": ");
            for (index, cookie) in cookies.iter().enumerate() {
                if index != 0 {
                    head.push_str("; ");
                }
                head.push_str(cookie);
            }
            head.push_str("\r\n");
            continue;
        }
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let mut request = head.into_bytes();
    let mut body_total = 0_u64;
    let first_chunk = match &outgoing.body_stream {
        Some(body) => match body
            .try_next_limited(&mut body_total, outgoing.body_limit)
            .await
        {
            Ok(chunk) => chunk,
            Err(error) => {
                // The request head is still observable by the upstream before
                // a streaming body error aborts the exchange.
                write_all_timed(stream, &request, timeout).await?;
                return Err(error);
            }
        },
        None => None,
    };
    let first_chunk_pending = match &outgoing.body_stream {
        Some(body) if outgoing.body_chunked => body.channel_has_pending().await,
        _ => false,
    };
    let mut chunked_complete = false;
    if let Some(chunk) = first_chunk.as_ref().filter(|chunk| !chunk.is_empty()) {
        append_request_body_chunk(&mut request, chunk, outgoing.body_chunked);
        if outgoing.body_chunked
            && outgoing
                .body_stream
                .as_ref()
                .is_some_and(|body| body.channel_ended())
            && !first_chunk_pending
        {
            request.extend_from_slice(b"0\r\n\r\n");
            chunked_complete = true;
        }
    }
    if outgoing.body_stream.is_none() {
        request.extend_from_slice(&outgoing.body);
    }
    write_all_timed(stream, &request, timeout).await?;
    if let Some(body) = &outgoing.body_stream {
        while let Some(chunk) = body
            .next_limited(&mut body_total, outgoing.body_limit)
            .await?
        {
            if chunk.is_empty() {
                continue;
            }
            if outgoing.body_chunked {
                let mut framed = Vec::with_capacity(chunk.len() + 16);
                append_request_body_chunk(&mut framed, &chunk, true);
                if body.channel_ended() && !body.channel_has_pending().await {
                    framed.extend_from_slice(b"0\r\n\r\n");
                    chunked_complete = true;
                }
                write_all_timed(stream, &framed, timeout).await?;
            } else {
                write_all_timed(stream, &chunk, timeout).await?;
            }
        }
        if outgoing.body_chunked && !chunked_complete {
            write_all_timed(stream, b"0\r\n\r\n", timeout).await?;
        }
    }
    flush_timed(stream, timeout).await
}

fn append_request_body_chunk(request: &mut Vec<u8>, chunk: &[u8], chunked: bool) {
    if chunked {
        request.extend_from_slice(format!("{:X}\r\n", chunk.len()).as_bytes());
    }
    request.extend_from_slice(chunk);
    if chunked {
        request.extend_from_slice(b"\r\n");
    }
}

async fn write_all_timed<S>(
    stream: &mut S,
    data: &[u8],
    timeout: Option<Duration>,
) -> io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    write_all_with_timeout(stream, data, timeout.unwrap_or(Duration::ZERO)).await
}

async fn flush_timed<S>(stream: &mut S, timeout: Option<Duration>) -> io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    flush_with_timeout(stream, timeout.unwrap_or(Duration::ZERO)).await
}
