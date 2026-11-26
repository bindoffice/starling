use super::*;

pub(super) fn request_from_h3_parts(
    parts: http::request::Parts,
    remote_addr: Arc<str>,
    local_addr: Arc<str>,
) -> Request {
    let mut headers: Vec<Header> = parts
        .headers
        .iter()
        .map(|(name, value)| Header {
            name: name.as_str().into(),
            value: crate::http::header_value_from_bytes(value.as_bytes()),
        })
        .collect();
    if !headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("host"))
        && let Some(authority) = parts.uri.authority()
    {
        headers.push(Header {
            name: "Host".into(),
            value: authority.as_str().to_string(),
        });
    }
    let target = parts
        .uri
        .path_and_query()
        .map(|path| path.as_str().to_string())
        .unwrap_or_else(|| parts.uri.to_string());
    Request {
        method: parts.method.as_str().to_string(),
        target,
        version: "HTTP/3.0".into(),
        headers: headers.into(),
        body: Vec::new(),
        body_stream: None,
        remote_addr,
        local_addr,
        keep_alive: true,
    }
}

pub(super) fn request_may_have_body(method: &str, content_length: Option<u64>) -> bool {
    content_length.is_some_and(|length| length != 0)
        || (content_length.is_none() && matches!(method, "POST" | "PUT" | "PATCH" | "DELETE"))
}
