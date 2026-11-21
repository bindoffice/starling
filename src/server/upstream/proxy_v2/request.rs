use super::*;

pub(super) fn build_h2_request(
    outgoing: &ProxyRequestMessage,
    proxy: &ProxyPassConfig,
) -> Result<http::Request<()>, ProxyFailure> {
    let authority = outgoing
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or(ProxyFailure::InvalidHeader)?;
    let uri = format!("{}://{}{}", proxy.scheme, authority, outgoing.path);
    let mut request = http::Request::builder()
        .method(outgoing.method.as_str())
        .uri(uri);
    for (name, value) in &outgoing.headers {
        if is_connection_header(name) || name.eq_ignore_ascii_case("host") {
            continue;
        }
        request = request.header(name.as_str(), value.as_str());
    }
    request.body(()).map_err(|_| ProxyFailure::InvalidHeader)
}

fn is_connection_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection" | "proxy-connection" | "keep-alive" | "transfer-encoding" | "upgrade"
    )
}
