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
    write_all_timed(stream, head.as_bytes(), timeout).await?;
    if let Some(body) = &outgoing.body_stream {
        let mut total = 0_u64;
        while let Some(chunk) = body.next_limited(&mut total, outgoing.body_limit).await? {
            if chunk.is_empty() {
                continue;
            }
            if outgoing.body_chunked {
                write_all_timed(stream, format!("{:X}\r\n", chunk.len()).as_bytes(), timeout)
                    .await?;
                write_all_timed(stream, &chunk, timeout).await?;
                write_all_timed(stream, b"\r\n", timeout).await?;
            } else {
                write_all_timed(stream, &chunk, timeout).await?;
            }
        }
        if outgoing.body_chunked {
            write_all_timed(stream, b"0\r\n\r\n", timeout).await?;
        }
    } else {
        write_all_timed(stream, &outgoing.body, timeout).await?;
    }
    flush_timed(stream, timeout).await
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
