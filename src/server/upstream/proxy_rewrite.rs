use super::*;

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
    let mut skip_headers: HashSet<String> = [
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
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    for header in &context.proxy_headers {
        skip_headers.insert(header.name.to_ascii_lowercase());
    }
    let mut headers = Vec::new();
    if context.proxy_pass_request_headers {
        for header in &request.headers {
            if skip_headers.contains(&header.name.to_ascii_lowercase()) {
                continue;
            }
            headers.push((header.name.clone(), header.value.clone()));
        }
    }
    for header in &context.proxy_headers {
        let value = eval_variables(&header.value, variables);
        if value.is_empty() {
            continue;
        }
        let name_lower = header.name.to_ascii_lowercase();
        if name_lower == "host" {
            host_header_set = true;
        }
        headers.push((header.name.clone(), value));
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

pub(in crate::server) const DEFAULT_PROXY_HIDE_HEADERS: &[&str] = &[
    "date",
    "server",
    "x-pad",
    "x-accel-expires",
    "x-accel-redirect",
    "x-accel-limit-rate",
    "x-accel-buffering",
    "x-accel-charset",
];

pub(in crate::server) fn filter_proxy_response_headers(
    headers: Vec<Header>,
    context: &EffectiveConfig,
) -> Vec<Header> {
    filter_proxy_headers(
        headers,
        &context.proxy_pass_headers,
        &context.proxy_hide_headers,
    )
}

pub(in crate::server) fn filter_proxy_trailer_headers(
    headers: Vec<Header>,
    pass_headers: &[String],
    hide_headers: &[String],
) -> Vec<Header> {
    filter_proxy_headers(headers, pass_headers, hide_headers)
}

fn filter_proxy_headers(
    headers: Vec<Header>,
    pass_headers: &[String],
    hide_headers: &[String],
) -> Vec<Header> {
    headers
        .into_iter()
        .filter(|header| {
            let name = header.name.to_ascii_lowercase();
            if matches!(
                name.as_str(),
                "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding"
            ) {
                return false;
            }
            if pass_headers
                .iter()
                .any(|header| header.eq_ignore_ascii_case(&name))
            {
                return true;
            }
            !DEFAULT_PROXY_HIDE_HEADERS.contains(&name.as_str())
                && !hide_headers
                    .iter()
                    .any(|header| header.eq_ignore_ascii_case(&name))
        })
        .collect()
}

pub(in crate::server) fn apply_proxy_redirect(
    response: &mut Response,
    proxy: &ProxyPassConfig,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    variables: &Variables,
) {
    for header in &mut response.headers {
        if header.name.eq_ignore_ascii_case("location") {
            if let Some(value) =
                proxy_redirect_value(&header.value, proxy, location, context, variables)
            {
                header.value = value;
            }
        } else if header.name.eq_ignore_ascii_case("refresh")
            && let Some(position) = header.value.to_ascii_lowercase().find("url=")
        {
            let value_start = position + "url=".len();
            if let Some(value) = proxy_redirect_value(
                &header.value[value_start..],
                proxy,
                location,
                context,
                variables,
            ) {
                header.value.replace_range(value_start.., &value);
            }
        }
    }
}

pub(in crate::server) fn proxy_redirect_value(
    value: &str,
    proxy: &ProxyPassConfig,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Option<String> {
    match &context.proxy_redirect {
        ProxyRedirectConfig::Off => None,
        ProxyRedirectConfig::Rules(rules) => rules.iter().find_map(|rule| match rule {
            ProxyRedirectRule::Default => {
                let (pattern, replacement) = default_proxy_redirect_rule(proxy, location)?;
                apply_proxy_redirect_rule(value, &pattern, &replacement, variables)
            }
            ProxyRedirectRule::Pattern {
                pattern,
                replacement,
            } => apply_proxy_redirect_rule(value, pattern, replacement, variables),
        }),
    }
}

pub(in crate::server) fn default_proxy_redirect_rule(
    proxy: &ProxyPassConfig,
    location: Option<&LocationConfig>,
) -> Option<(String, String)> {
    if proxy.raw.contains('$') {
        return None;
    }
    if proxy.uri.is_none() {
        return Some((format!("{}/", proxy.raw), "/".into()));
    }
    let location = location?;
    let replacement = match &location.matcher {
        LocationMatcher::Exact(path)
        | LocationMatcher::Prefix(path)
        | LocationMatcher::PrefixNoRegex(path) => path.clone(),
        LocationMatcher::Regex(_) | LocationMatcher::Named(_) => return None,
    };
    Some((proxy.raw.clone(), replacement))
}

pub(in crate::server) fn apply_proxy_redirect_rule(
    value: &str,
    pattern: &str,
    replacement: &str,
    variables: &Variables,
) -> Option<String> {
    if let Some(pattern) = pattern.strip_prefix('~') {
        let (pattern, case_insensitive) = match pattern.strip_prefix('*') {
            Some(pattern) => (pattern, true),
            None => (pattern, false),
        };
        let regex = crate::config::RegexBuilder::new(pattern)
            .case_insensitive(case_insensitive)
            .build()
            .ok()?;
        let captures = regex.captures(value)?;
        return Some(expand_captures(replacement, &captures, variables));
    }
    let pattern = eval_variables(pattern, variables);
    let replacement = eval_variables(replacement, variables);
    if value.starts_with(&pattern) {
        return Some(format!("{replacement}{}", &value[pattern.len()..]));
    }
    None
}

pub(in crate::server) fn apply_proxy_cookie_rewrites(
    response: &mut Response,
    context: &EffectiveConfig,
    variables: &Variables,
) {
    for header in &mut response.headers {
        if !header.name.eq_ignore_ascii_case("set-cookie") {
            continue;
        }
        let mut attributes = parse_proxy_cookie(&header.value);
        let mut changed = false;
        for attribute in attributes.iter_mut().skip(1) {
            let Some(cookie_value) = attribute.value.as_deref() else {
                continue;
            };
            let rewritten = if attribute.key.eq_ignore_ascii_case("domain") {
                apply_proxy_cookie_domain_rule(
                    cookie_value,
                    &context.proxy_cookie_domains,
                    variables,
                )
            } else if attribute.key.eq_ignore_ascii_case("path") {
                context.proxy_cookie_paths.iter().find_map(|rule| {
                    apply_proxy_redirect_rule(
                        cookie_value,
                        &rule.pattern,
                        &rule.replacement,
                        variables,
                    )
                })
            } else {
                None
            };
            if let Some(rewritten) = rewritten {
                attribute.value = Some(rewritten);
                changed = true;
            }
        }
        changed |=
            apply_proxy_cookie_flags(&mut attributes, &context.proxy_cookie_flags, variables);
        if changed {
            header.value = render_proxy_cookie(&attributes);
        }
    }
}

pub(in crate::server) fn apply_proxy_cookie_domain_rule(
    value: &str,
    rules: &[crate::config::ProxyCookieRule],
    variables: &Variables,
) -> Option<String> {
    for rule in rules {
        if let Some(pattern) = rule.pattern.strip_prefix('~') {
            // Cookie domains are DNS names, so nginx always matches this
            // regular expression case-insensitively.
            let pattern = pattern.strip_prefix('*').unwrap_or(pattern);
            let regex = crate::config::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .ok()?;
            let Some(captures) = regex.captures(value) else {
                continue;
            };
            return Some(expand_captures(&rule.replacement, &captures, variables));
        }
        let pattern = eval_variables(&rule.pattern, variables);
        let pattern = pattern.strip_prefix('.').unwrap_or(&pattern);
        let leading_dot = value.starts_with('.');
        let candidate = value.strip_prefix('.').unwrap_or(value);
        if candidate.eq_ignore_ascii_case(pattern) {
            let replacement = eval_variables(&rule.replacement, variables);
            let replacement = replacement.strip_prefix('.').unwrap_or(&replacement);
            return Some(format!(
                "{}{replacement}",
                if leading_dot { "." } else { "" }
            ));
        }
    }
    None
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
        });
    }
    if targets.is_empty() {
        targets.push(ResolvedUpstreamTarget {
            host,
            port,
            peer: None,
            check_health: false,
        });
    }
    targets
}

pub(in crate::server) fn resolve_upstream_config(
    config: &UpstreamConfig,
    peer: Option<IpAddr>,
    hash_key: Option<&str>,
) -> Vec<ResolvedUpstreamTarget> {
    let primary_peers = config
        .servers
        .iter()
        .filter(|server| !server.backup && !server.down)
        .count();
    let primary_health = primary_peers != 1;
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
