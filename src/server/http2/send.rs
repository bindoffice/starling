use super::*;

pub(in crate::server) async fn send_h2_response(
    mut respond: SendResponse<Bytes>,
    mut response: Response,
    request_method: &str,
    server_tokens: ServerTokens,
    chunk_size: usize,
) -> io::Result<()> {
    prepare_response_headers(&mut response, server_tokens);
    let mut builder = HttpResponse::builder()
        .status(StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR));
    for header in &response.headers {
        if is_h2_connection_header(&header.name) {
            continue;
        }
        let Ok(name) = http::header::HeaderName::from_bytes(header.name.as_bytes()) else {
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
        return Ok(());
    }

    let body = std::mem::replace(&mut response.body, ResponseBody::Empty);
    let mut trailers =
        match super::body::send_h2_body(&mut send, body, has_trailers, chunk_size).await {
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

fn h2_header_value(
    value: &str,
) -> Result<http::header::HeaderValue, http::header::InvalidHeaderValue> {
    if value
        .as_bytes()
        .iter()
        .any(|byte| matches!(byte, b'\r' | b'\n'))
    {
        http::header::HeaderValue::from_str(&sanitize_header_value(value))
    } else {
        http::header::HeaderValue::from_str(value)
    }
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
    }
}
