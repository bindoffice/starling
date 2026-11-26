use super::*;

pub(in crate::server) async fn send_h2_response(
    respond: SendResponse<Bytes>,
    response: Response,
    request_method: &str,
    server_tokens: ServerTokens,
    chunk_size: usize,
) -> io::Result<()> {
    send_h2_response_inner(
        respond,
        response,
        request_method,
        server_tokens,
        chunk_size,
        false,
        false,
    )
    .await
}

pub(in crate::server) async fn send_h2_response_with_sendfile(
    respond: SendResponse<Bytes>,
    response: Response,
    request_method: &str,
    server_tokens: ServerTokens,
    chunk_size: usize,
    sendfile: bool,
) -> io::Result<()> {
    send_h2_response_inner(
        respond,
        response,
        request_method,
        server_tokens,
        chunk_size,
        sendfile,
        false,
    )
    .await
}

pub(in crate::server) async fn send_h2_response_discarding_request_body(
    respond: SendResponse<Bytes>,
    response: Response,
    request_method: &str,
    server_tokens: ServerTokens,
    chunk_size: usize,
) -> io::Result<()> {
    send_h2_response_inner(
        respond,
        response,
        request_method,
        server_tokens,
        chunk_size,
        false,
        true,
    )
    .await
}

async fn send_h2_response_inner(
    mut respond: SendResponse<Bytes>,
    mut response: Response,
    request_method: &str,
    server_tokens: ServerTokens,
    chunk_size: usize,
    sendfile: bool,
    discard_request_body: bool,
) -> io::Result<()> {
    prepare_response_headers(&mut response, server_tokens);
    let mut builder = HttpResponse::builder()
        .status(StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR));
    for header in &response.headers {
        if is_h2_connection_header(&header.name) {
            continue;
        }
        let Some(name) = h2_header_name(&header.name) else {
            continue;
        };
        let Ok(value) = h2_header_value(&header.value) else {
            continue;
        };
        builder = builder.header(name, value);
    }
    let response_head = builder.body(()).map_err(io::Error::other)?;
    let is_head = request_method.eq_ignore_ascii_case("HEAD");
    let has_body = !is_head && !response.body.is_empty();
    let has_trailers = !is_head
        && !response.trailers.is_empty()
        && response.status >= 200
        && !matches!(response.status, 204 | 304);
    let has_output = has_body || has_trailers;
    let mut send = respond
        .send_response(response_head, !has_output)
        .map_err(io::Error::other)?;
    if !has_output {
        if discard_request_body {
            send.send_reset(h2::Reason::NO_ERROR);
        }
        return Ok(());
    }

    let body = std::mem::replace(&mut response.body, ResponseBody::Empty);
    let mut trailers = match super::body::send_h2_body(
        &mut send,
        body,
        has_trailers,
        chunk_size,
        sendfile,
    )
    .await
    {
        Ok(trailers) => trailers,
        Err(error) => {
            send.send_reset(h2_reset_reason(&error));
            return Err(error);
        }
    };
    if has_trailers {
        trailers.extend(response.trailers.iter().cloned());
    }
    if !trailers.is_empty() {
        super::body::send_h2_trailers(&mut send, &trailers)?;
    }
    if discard_request_body {
        send.send_reset(h2::Reason::NO_ERROR);
    }
    Ok(())
}

fn h2_reset_reason(error: &io::Error) -> h2::Reason {
    match error.kind() {
        io::ErrorKind::BrokenPipe
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionReset => h2::Reason::CANCEL,
        _ => h2::Reason::INTERNAL_ERROR,
    }
}

pub(in crate::server) fn is_h2_connection_header(name: &str) -> bool {
    [
        "connection",
        "keep-alive",
        "proxy-connection",
        "transfer-encoding",
        "upgrade",
        "te",
        "trailer",
    ]
    .iter()
    .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

pub(in crate::server) fn h2_header_name(name: &str) -> Option<http::header::HeaderName> {
    const STATIC_NAMES: &[&str] = &[
        "content-type",
        "content-length",
        "date",
        "server",
        "last-modified",
        "etag",
        "cache-control",
        "location",
        "set-cookie",
        "vary",
        "accept-ranges",
        "content-disposition",
        "content-encoding",
        "content-language",
        "content-location",
        "content-range",
        "expires",
        "p3p",
        "www-authenticate",
    ];
    if let Some(static_name) = STATIC_NAMES
        .iter()
        .find(|candidate| name.eq_ignore_ascii_case(candidate))
    {
        return Some(http::header::HeaderName::from_static(static_name));
    }
    http::header::HeaderName::from_bytes(name.as_bytes()).ok()
}

pub(in crate::server) fn h2_header_value(
    value: &str,
) -> Result<http::header::HeaderValue, http::header::InvalidHeaderValue> {
    let bytes = crate::http::header_value_bytes(value);
    if bytes
        .iter()
        .all(|byte| matches!(byte, 32..=126 | 128..=255 | b'\t'))
    {
        return http::header::HeaderValue::from_bytes(&bytes);
    }
    // Header values cannot contain NUL. Other control bytes are sanitized to
    // preserve the previous wire behavior without constructing an unchecked
    // HeaderValue.
    if bytes.contains(&0) {
        return http::header::HeaderValue::from_bytes(&bytes);
    }
    let sanitized: Vec<u8> = bytes
        .iter()
        .map(|byte| match byte {
            b'\r' | b'\n' | 0..=8 | 11..=31 | 127 => b' ',
            byte => *byte,
        })
        .collect();
    http::header::HeaderValue::from_bytes(&sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_connection_headers_case_insensitively() {
        assert!(is_h2_connection_header("Connection"));
        assert!(is_h2_connection_header("TRANSFER-ENCODING"));
        assert!(!is_h2_connection_header("content-type"));
    }

    #[test]
    fn sanitizes_only_header_values_with_line_breaks() {
        assert_eq!(
            h2_header_value("safe value").unwrap().to_str().unwrap(),
            "safe value"
        );
        assert_eq!(
            h2_header_value("first\r\nsecond")
                .unwrap()
                .to_str()
                .unwrap(),
            "first  second"
        );
        assert!(h2_header_value("invalid\0value").is_err());
    }

    #[test]
    fn sanitizes_other_control_bytes_in_header_values() {
        assert_eq!(
            h2_header_value("before\u{1}after").unwrap().as_bytes(),
            b"before after"
        );
        assert_eq!(
            h2_header_value("before\u{7f}after").unwrap().as_bytes(),
            b"before after"
        );
    }
}
