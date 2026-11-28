use super::*;

pub(in crate::server) fn can_reuse_proxy_connection(
    response: &UpstreamResponse,
    outgoing: &ProxyRequestMessage,
    version: &str,
) -> bool {
    if version != "1.1" || response.version != "HTTP/1.1" || response.status == 101 {
        return false;
    }
    if header_has_token(
        outgoing
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        "connection",
        "close",
    ) || header_has_token(
        response
            .headers
            .iter()
            .map(|header| (header.name.as_str(), header.value.as_str())),
        "connection",
        "close",
    ) || header_has_token(
        outgoing
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        "connection",
        "upgrade",
    ) || header_has_token(
        response
            .headers
            .iter()
            .map(|header| (header.name.as_str(), header.value.as_str())),
        "connection",
        "upgrade",
    ) {
        return false;
    }
    let no_body =
        response.status == 204 || response.status == 304 || (100..200).contains(&response.status);
    let chunked = response.headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("transfer-encoding")
            && header
                .value
                .split(',')
                .next_back()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("chunked"))
    });
    let content_length = response.headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("content-length")
            && header.value.trim().parse::<u64>().is_ok()
    });
    no_body || chunked || content_length
}

pub(super) fn deferred_stream_connection(
    eligible: bool,
) -> (
    Option<tokio::sync::oneshot::Sender<UpstreamConnection>>,
    Option<tokio::sync::oneshot::Receiver<UpstreamConnection>>,
) {
    if eligible {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        (Some(sender), Some(receiver))
    } else {
        (None, None)
    }
}

pub(super) fn can_reuse_stream_head(
    outgoing: &ProxyRequestMessage,
    request_version: &str,
    head: &UpstreamResponseHead,
) -> bool {
    if request_version != "1.1"
        || head.version != "HTTP/1.1"
        || head.status == 101
        || !matches!(
            head.body_kind,
            UpstreamBodyKind::ContentLength(_) | UpstreamBodyKind::Chunked
        )
    {
        return false;
    }
    !header_has_token(
        outgoing
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        "connection",
        "close",
    ) && !header_has_token(
        outgoing
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        "connection",
        "upgrade",
    ) && !header_has_token(
        head.headers
            .iter()
            .map(|header| (header.name.as_str(), header.value.as_str())),
        "connection",
        "close",
    ) && !header_has_token(
        head.headers
            .iter()
            .map(|header| (header.name.as_str(), header.value.as_str())),
        "connection",
        "upgrade",
    )
}

pub(in crate::server) fn spawn_stream_connection_reuse(
    receiver: tokio::sync::oneshot::Receiver<UpstreamConnection>,
    worker: usize,
    pool: Arc<UpstreamKeepalivePool>,
    upstream: Arc<str>,
    config: UpstreamKeepaliveConfig,
) {
    tokio::spawn(async move {
        if let Ok(connection) = receiver.await {
            pool.put_for(worker, &upstream, &config, connection);
        }
    });
}

pub(in crate::server) fn header_has_token<'a>(
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
    name: &str,
    token: &str,
) -> bool {
    headers
        .into_iter()
        .filter(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .flat_map(|(_, value)| value.split(','))
        .any(|value| value.trim().eq_ignore_ascii_case(token))
}
