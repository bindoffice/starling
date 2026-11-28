use super::*;
use crate::config::{HeaderValue, UpstreamMethod};

mod response;

pub(in crate::server) use response::*;

pub(in crate::server) fn build_proxy_request(
    proxy: &ProxyPassConfig,
    request: &Request,
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    variables: &Variables,
) -> ProxyRequestMessage {
    let method = context
        .proxy_method
        .as_deref()
        .map(|method| eval_variables(method, variables))
        .unwrap_or_else(|| request.method.clone());
    let path = proxy_path(proxy, env, location, variables);
    let body = match &context.proxy_set_body {
        Some(body) => eval_variables(body, variables).into_bytes(),
        None if context.proxy_pass_request_body && request.body_file().is_none() => {
            request.body.clone()
        }
        None => Vec::new(),
    };
    let body_stream = (context.proxy_set_body.is_none() && context.proxy_pass_request_body)
        .then(|| request_body_stream_for_upstream(request, context))
        .flatten()
        .cloned();
    let file_backed_body = body_stream
        .as_ref()
        .is_some_and(crate::http::RequestBodyStream::is_file_backed);
    let body_chunked = body_stream.is_some()
        && !file_backed_body
        && request.header("content-length").is_none()
        && (request.version == "HTTP/2.0"
            || request.header("transfer-encoding").is_some_and(|value| {
                value
                    .split(',')
                    .next_back()
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case("chunked"))
            }));
    let mut host_header_set = false;
    let mut headers = Vec::with_capacity(request.headers.len() + context.proxy_headers.len() + 1);
    if context.proxy_pass_request_headers {
        for header in request.headers.iter() {
            if should_skip_proxy_request_header(&header.name, &context.proxy_headers) {
                continue;
            }
            headers.push((header.name.to_string(), header.value.clone()));
        }
    }
    for header in &context.proxy_headers {
        let value = eval_variables(&header.value, variables);
        if value.is_empty() {
            continue;
        }
        if header.name.eq_ignore_ascii_case("host") {
            host_header_set = true;
        }
        headers.push((header.name.to_string(), value));
    }
    if !host_header_set {
        headers.push(("Host".into(), variables.proxy_host.clone()));
    }
    if body_chunked {
        headers.push(("Transfer-Encoding".into(), "chunked".into()));
    } else if body_stream.is_some() {
        let length = body_stream
            .as_ref()
            .and_then(crate::http::RequestBodyStream::file_len)
            .map(|length| length.to_string())
            .or_else(|| request.header("content-length").map(str::to_string));
        if let Some(length) = length {
            headers.push(("Content-Length".into(), length.trim().to_string()));
        }
    } else if context.proxy_set_body.is_some()
        || (context.proxy_pass_request_body
            && (request.body_file().is_some()
                || !request.body.is_empty()
                || request.header("content-length").is_some()))
    {
        headers.push(("Content-Length".into(), body.len().to_string()));
    }
    ProxyRequestMessage {
        method,
        path,
        headers,
        body,
        body_stream,
        body_chunked,
        body_limit: context.client_max_body_size,
    }
}

fn should_skip_proxy_request_header(name: &str, configured: &[HeaderValue]) -> bool {
    const SKIPPED: &[&str] = &[
        "host",
        "connection",
        "content-length",
        "proxy-connection",
        "keep-alive",
        "transfer-encoding",
        "te",
        "trailer",
        "upgrade",
        "expect",
    ];
    SKIPPED
        .iter()
        .any(|skipped| name.eq_ignore_ascii_case(skipped))
        || configured
            .iter()
            .any(|header| name.eq_ignore_ascii_case(&header.name))
}

pub(in crate::server) async fn resolve_proxy_targets(
    proxy: &ProxyPassConfig,
    http: &HttpConfig,
    variables: &Variables,
    peer: Option<IpAddr>,
) -> Vec<ResolvedUpstreamTarget> {
    if let Some(upstream) = &proxy.upstream
        && let Some(config) = http.upstreams.get(upstream)
    {
        let hash_key = config
            .hash_key
            .as_deref()
            .map(|key| eval_variables(key, variables));
        return resolve_upstream_config(config, peer, hash_key.as_deref());
    }
    let host = eval_variables(&proxy.host, variables);
    let (host, port) = proxy_runtime_authority(&host, proxy.port);
    resolve_implicit_proxy_targets(host, port).await
}

fn proxy_runtime_authority(value: &str, default_port: u16) -> (String, u16) {
    if value.starts_with("unix:") {
        return (value.to_string(), 0);
    }
    if let Some(rest) = value.strip_prefix('[')
        && let Some((host, suffix)) = rest.split_once(']')
    {
        let port = suffix
            .strip_prefix(':')
            .and_then(|port| port.parse().ok())
            .unwrap_or(default_port);
        return (format!("[{host}]"), port);
    }
    if let Some((host, port)) = value.rsplit_once(':')
        && !host.contains(':')
        && let Ok(port) = port.parse()
    {
        return (host.to_string(), port);
    }
    (value.to_string(), default_port)
}

async fn resolve_implicit_proxy_targets(host: String, port: u16) -> Vec<ResolvedUpstreamTarget> {
    if host.starts_with("unix:") || host.parse::<IpAddr>().is_ok() {
        return vec![ResolvedUpstreamTarget {
            host,
            port,
            peer: None,
            check_health: false,
            record_timing: false,
        }];
    }
    let lookup_host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(&host);
    let Ok(addresses) = tokio::net::lookup_host((lookup_host, port)).await else {
        return vec![ResolvedUpstreamTarget {
            host,
            port,
            peer: None,
            check_health: false,
            record_timing: false,
        }];
    };
    let mut targets = Vec::new();
    for address in addresses {
        let host = match address.ip() {
            IpAddr::V4(address) => address.to_string(),
            IpAddr::V6(address) => format!("[{address}]"),
        };
        if targets.iter().any(|target: &ResolvedUpstreamTarget| {
            target.host == host && target.port == address.port()
        }) {
            continue;
        }
        targets.push(ResolvedUpstreamTarget {
            host,
            port: address.port(),
            peer: None,
            check_health: false,
            record_timing: false,
        });
    }
    if targets.is_empty() {
        targets.push(ResolvedUpstreamTarget {
            host,
            port,
            peer: None,
            check_health: false,
            record_timing: false,
        });
    }
    targets
}

pub(in crate::server) fn resolve_upstream_config(
    config: &UpstreamConfig,
    peer: Option<IpAddr>,
    hash_key: Option<&str>,
) -> Vec<ResolvedUpstreamTarget> {
    if config.servers.len() == 1 {
        let server = &config.servers[0];
        if !server.is_available() {
            return Vec::new();
        }
        return parse_upstream_address(&server.address)
            .map(|(host, port)| {
                vec![ResolvedUpstreamTarget {
                    host,
                    port,
                    check_health: server.backup,
                    record_timing: matches!(config.method, UpstreamMethod::LeastTime(_)),
                    peer: Some(server.clone()),
                }]
            })
            .unwrap_or_default();
    }
    let primary_peers = config
        .servers
        .iter()
        .filter(|server| !server.backup && !server.down)
        .count();
    let primary_health = primary_peers != 1;
    let record_timing = matches!(config.method, UpstreamMethod::LeastTime(_));
    config
        .select_candidates_with(peer, hash_key)
        .into_iter()
        .filter_map(|server| {
            let check_health = server.backup || primary_health;
            parse_upstream_address(&server.address).map(|(host, port)| ResolvedUpstreamTarget {
                host,
                port,
                peer: Some(server.clone()),
                check_health,
                record_timing,
            })
        })
        .collect()
}

pub(in crate::server) fn request_peer(request: &Request) -> Option<IpAddr> {
    request
        .remote_addr
        .parse::<SocketAddr>()
        .ok()
        .map(|address| address.ip())
}

pub(in crate::server) fn parse_upstream_address(value: &str) -> Option<(String, u16)> {
    if value.starts_with("unix:") {
        return Some((value.to_string(), 0));
    }
    if let Ok(address) = value.parse::<SocketAddr>() {
        return Some((address.ip().to_string(), address.port()));
    }
    if let Some((host, port)) = value.rsplit_once(':') {
        return Some((
            host.trim_matches(['[', ']']).to_string(),
            port.parse().ok()?,
        ));
    }
    Some((value.to_string(), 80))
}

pub(in crate::server) fn proxy_path(
    proxy: &ProxyPassConfig,
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    variables: &Variables,
) -> String {
    let mut path = if !env.valid_location {
        utf8_percent_encode(&env.uri, PROXY_PATH_ENCODE_SET).to_string()
    } else if let Some(uri) = &proxy.uri {
        let uri = eval_variables(uri, variables);
        if uri.contains('$') {
            uri
        } else {
            let suffix = match location.map(|location| &location.matcher) {
                Some(LocationMatcher::Prefix(prefix))
                | Some(LocationMatcher::PrefixNoRegex(prefix)) => {
                    env.uri.strip_prefix(prefix).unwrap_or_default()
                }
                _ => "",
            };
            format!(
                "{uri}{}",
                utf8_percent_encode(suffix, PROXY_PATH_ENCODE_SET)
            )
        }
    } else if let Some(unparsed_uri) = &env.unparsed_uri {
        return unparsed_uri.clone();
    } else {
        utf8_percent_encode(&env.uri, PROXY_PATH_ENCODE_SET).to_string()
    };
    if let Some(args) = &env.args
        && !args.is_empty()
        && !path.contains('?')
    {
        path.push('?');
        path.push_str(args);
    }
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    path
}
