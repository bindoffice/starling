use super::*;

pub(in crate::server) struct GrpcOutgoingRequest {
    pub(in crate::server) request: http::Request<()>,
    pub(in crate::server) body: Vec<u8>,
    pub(in crate::server) body_stream: Option<crate::http::RequestBodyStream>,
}

pub(in crate::server) fn build_grpc_request(
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    variables: &Variables,
    authority: &str,
    tls: bool,
) -> Result<GrpcOutgoingRequest, ProxyFailure> {
    let body_stream = request_body_stream_for_upstream(request, context).cloned();
    let body = if body_stream.is_some() {
        Vec::new()
    } else {
        request.body.clone()
    };
    let path = grpc_path(env);
    let custom: HashSet<String> = context
        .grpc
        .headers
        .iter()
        .map(|header| header.name.to_ascii_lowercase())
        .collect();
    let configured_authority = context
        .grpc
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("host"))
        .map(|header| eval_variables(&header.value, variables))
        .filter(|value| !value.is_empty());
    let authority = configured_authority.as_deref().unwrap_or(authority);
    let scheme = if tls { "https" } else { "http" };
    let uri = format!("{scheme}://{authority}{path}");
    let mut outgoing = http::Request::builder()
        .method(request.method.as_str())
        .uri(uri);

    let mut sent: HashSet<String> = [
        "host",
        "connection",
        "content-length",
        "proxy-connection",
        "transfer-encoding",
        "keep-alive",
        "expect",
        "upgrade",
        "te",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    sent.extend(custom.iter().cloned());

    if !custom.contains("te")
        && header_has_token(
            request
                .headers
                .iter()
                .map(|header| (header.name.as_str(), header.value.as_str())),
            "te",
            "trailers",
        )
    {
        outgoing = outgoing.header("te", "trailers");
    }
    let content_length = if let Some(stream) = &body_stream {
        stream
            .file_len()
            .map(|length| length.to_string())
            .or_else(|| request.header("content-length").map(str::to_string))
    } else if !body.is_empty() || request.header("content-length").is_some() {
        Some(body.len().to_string())
    } else {
        None
    };
    if !custom.contains("content-length")
        && let Some(length) = content_length
    {
        outgoing = outgoing.header("content-length", length);
    }
    for header in &request.headers {
        let lower = header.name.to_ascii_lowercase();
        if sent.contains(&lower) {
            continue;
        }
        outgoing = outgoing.header(header.name.as_str(), header.value.as_str());
    }
    for header in &context.grpc.headers {
        if header.name.eq_ignore_ascii_case("host") {
            continue;
        }
        let value = eval_variables(&header.value, variables);
        if value.is_empty() {
            continue;
        }
        outgoing = outgoing.header(header.name.as_str(), value);
    }
    let request = outgoing.body(()).map_err(|_| ProxyFailure::InvalidHeader)?;
    Ok(GrpcOutgoingRequest {
        request,
        body,
        body_stream,
    })
}

fn grpc_path(env: &RequestEnv) -> String {
    if let Some(unparsed) = &env.unparsed_uri {
        if unparsed.starts_with('/') {
            return unparsed.clone();
        }
        return format!("/{unparsed}");
    }
    let mut path = utf8_percent_encode(&env.uri, PROXY_PATH_ENCODE_SET).to_string();
    if let Some(args) = &env.args
        && !args.is_empty()
    {
        path.push('?');
        path.push_str(args);
    }
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    path
}
