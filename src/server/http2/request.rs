use super::*;
use crate::http::valid_host;

pub(super) fn h2_content_length(headers: &http::HeaderMap) -> io::Result<Option<u64>> {
    let mut content_length = None;
    for value in headers.get_all(http::header::CONTENT_LENGTH).iter() {
        let parsed = value
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length"))?;
        if content_length.is_some_and(|previous| previous != parsed) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "conflicting Content-Length headers",
            ));
        }
        content_length = Some(parsed);
    }
    Ok(content_length)
}

pub(super) fn h2_header_field_too_large(parts: &http::request::Parts, limit: usize) -> bool {
    if limit == 0 {
        return false;
    }
    let uri = &parts.uri;
    let uri_field_too_large = uri.scheme_str().is_some_and(|value| value.len() > limit)
        || uri
            .authority()
            .is_some_and(|value| value.as_str().len() > limit)
        || uri
            .path_and_query()
            .is_some_and(|value| value.as_str().len() > limit);
    // The h2 crate exposes only the aggregate decoded header-list size. Check each
    // decoded field here to preserve Nginx's per-field large_client_header_buffers
    // limit before the request enters routing or application code.
    uri_field_too_large
        || parts.method.as_str().len() > limit
        || parts
            .headers
            .iter()
            .any(|(name, value)| name.as_str().len() > limit || value.as_bytes().len() > limit)
}

pub(super) fn h2_header_list_too_large(parts: &http::request::Parts, limit: usize) -> bool {
    if limit == 0 {
        return false;
    }
    let uri = &parts.uri;
    let mut size = ":method".len().saturating_add(parts.method.as_str().len());
    if let Some(scheme) = uri.scheme_str() {
        size = size
            .saturating_add(":scheme".len())
            .saturating_add(scheme.len());
    }
    if let Some(authority) = uri.authority() {
        size = size
            .saturating_add(":authority".len())
            .saturating_add(authority.as_str().len());
    }
    if let Some(path) = uri.path_and_query() {
        size = size
            .saturating_add(":path".len())
            .saturating_add(path.as_str().len());
    }
    size = parts.headers.iter().fold(size, |total, (name, value)| {
        total
            .saturating_add(name.as_str().len())
            .saturating_add(value.as_bytes().len())
    });
    size > limit
}

pub(super) fn invalid_h2_request_headers(parts: &http::request::Parts) -> bool {
    let uri = &parts.uri;
    if !parts
        .method
        .as_str()
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || matches!(byte, b'_' | b'-'))
    {
        return true;
    }
    let authority = uri.authority().map(http::uri::Authority::as_str);
    if parts.method == http::Method::CONNECT {
        let Some(authority) = authority else {
            return true;
        };
        if !valid_host(authority) {
            return true;
        }
        let mut host_values = parts.headers.get_all(http::header::HOST).iter();
        let Some(host) = host_values.next() else {
            return false;
        };
        return host_values.next().is_some()
            || !host.as_bytes().eq_ignore_ascii_case(authority.as_bytes());
    }
    if parts
        .extensions
        .get::<h2::server::MalformedRequest>()
        .is_some()
    {
        return true;
    }

    if uri.scheme().is_none() && parts.extensions.get::<http::uri::Scheme>().is_none() {
        return true;
    }
    let Some(path) = uri.path_and_query().map(http::uri::PathAndQuery::as_str) else {
        return true;
    };
    if !path.starts_with('/') && !(parts.method == http::Method::OPTIONS && path == "*") {
        return true;
    }

    let host_values: Vec<&http::HeaderValue> =
        parts.headers.get_all(http::header::HOST).iter().collect();
    if host_values.len() > 1 {
        return true;
    }
    let host = host_values.first().map(|value| value.as_bytes());
    match (authority, host) {
        (Some(authority), Some(host)) => {
            !valid_host(authority) || !host.eq_ignore_ascii_case(authority.as_bytes())
        }
        (Some(authority), None) => !valid_host(authority),
        (None, Some(host)) => {
            let Ok(host) = std::str::from_utf8(host) else {
                return true;
            };
            !valid_host(host)
        }
        (None, None) => true,
    }
}

pub(super) fn receive_header_list_limit(limit: u32) -> u32 {
    // The h2 crate enforces the advertised limit itself and answers with
    // 431/RST_STREAM. Keep a bounded decode ceiling here, then enforce Nginx's
    // aggregate limit after decoding so protocol violations become
    // GOAWAY(ENHANCE_YOUR_CALM). The 4x ceiling matches the crate's own abuse
    // guard and prevents unbounded header allocation.
    limit.saturating_mul(4)
}

pub(super) fn request_from_h2_parts(
    parts: http::request::Parts,
    body: Vec<u8>,
    remote_addr: Arc<str>,
    local_addr: Arc<str>,
) -> (Request, Option<u64>) {
    let header_block_size = parts
        .extensions
        .get::<h2::ext::HeaderBlockSize>()
        .map(|size| size.0 as u64);
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
    (
        Request {
            method: parts.method.as_str().to_string(),
            target,
            version: "HTTP/2.0".into(),
            headers: headers.into(),
            body,
            body_stream: None,
            remote_addr,
            local_addr,
            keep_alive: true,
        },
        header_block_size,
    )
}
