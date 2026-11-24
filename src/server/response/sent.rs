use super::*;

pub(in crate::server) fn sync_sent_response_variables(
    response: &Response,
    request: &Request,
    variables: &mut Variables,
    keep_alive: bool,
    keepalive_header_timeout: Option<u64>,
    chunked_transfer_encoding: bool,
) {
    let chunked = crate::http::response_uses_chunked_encoding(
        response,
        &request.method,
        &request.version,
        chunked_transfer_encoding,
    );
    variables.sent_headers.clone_from(&response.headers);
    if chunked {
        variables
            .sent_headers
            .retain(|header| !header.name.eq_ignore_ascii_case("content-length"));
    }
    variables.sent_trailers.clone_from(&response.trailers);
    variables.sent_connection = if response.status == 101 {
        "upgrade".into()
    } else if keep_alive {
        "keep-alive".into()
    } else {
        "close".into()
    };
    variables.sent_keep_alive = keepalive_header_timeout
        .map(|timeout| format!("timeout={timeout}"))
        .unwrap_or_default();
    variables.sent_transfer_encoding = if chunked {
        "chunked".into()
    } else {
        String::new()
    };
}

pub(in crate::server) fn response_wire_body_len(
    response: &Response,
    request_method: &str,
    chunked: bool,
) -> u64 {
    if request_method.eq_ignore_ascii_case("HEAD") {
        return 0;
    }
    if !chunked {
        return response.body_len();
    }

    let body_len = response.body_len();
    let mut total: u64 = if body_len == 0 {
        5
    } else {
        format!("{body_len:x}\r\n").len() as u64 + body_len + 2 + 5
    };
    for trailer in &response.trailers {
        total = total
            .saturating_add(trailer.name.len() as u64)
            .saturating_add(2)
            .saturating_add(trailer.value.len() as u64)
            .saturating_add(2);
    }
    total
}
