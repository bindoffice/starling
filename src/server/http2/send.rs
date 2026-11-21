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
        let value = sanitize_header_value(&header.value);
        let Ok(value) = http::header::HeaderValue::from_str(&value) else {
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
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-connection"
            | "transfer-encoding"
            | "upgrade"
            | "te"
            | "trailer"
    )
}
