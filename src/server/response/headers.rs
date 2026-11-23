use super::*;

pub(in crate::server) fn apply_response_headers(
    response: &mut Response,
    context: &EffectiveConfig,
    request: &Request,
    variables: &mut Variables,
) {
    variables.status = response.status;
    variables.sent_headers.clone_from(&response.headers);
    let status_allows = matches!(
        response.status,
        200 | 201 | 204 | 206 | 301 | 302 | 303 | 304 | 307 | 308
    );
    for header in &context.add_headers {
        if status_allows || header.always {
            let value = eval_variables(&header.value, variables);
            let name = header.name.to_ascii_lowercase();
            if value.is_empty() {
                if matches!(name.as_str(), "etag" | "last-modified") {
                    response.remove_header(&header.name);
                }
            } else if matches!(name.as_str(), "etag" | "last-modified") {
                response.set_header(&header.name, value);
            } else {
                if name == "accept-ranges"
                    && response.status == 200
                    && value.eq_ignore_ascii_case("bytes")
                {
                    response.enable_ranges(false);
                }
                response.add_header(&header.name, value);
            }
            variables.sent_headers.clone_from(&response.headers);
        }
    }
    if request.method.eq_ignore_ascii_case("HEAD") && response.header("Content-Length").is_none() {
        response.set_content_length();
    }
}

pub(in crate::server) fn apply_redirect_location(
    response: &mut Response,
    context: &EffectiveConfig,
    server: &ServerConfig,
    request: &Request,
    local: &ListenAddress,
    tls: bool,
) {
    if !context.absolute_redirect {
        return;
    }
    let Some(location) = response.header("Location").map(str::to_string) else {
        return;
    };
    if !location.starts_with('/') {
        return;
    }
    let configured_name = server
        .names
        .first()
        .map(server_name_value)
        .filter(|name| !name.is_empty());
    let request_host = request.host_without_port();
    let request_host = (!request_host.is_empty()).then(|| request_host.into_owned());
    let host = if context.server_name_in_redirect {
        configured_name.or(request_host)
    } else {
        request_host
    }
    .unwrap_or_else(|| match local {
        ListenAddress::Inet(address) => match address.ip() {
            IpAddr::V4(address) => address.to_string(),
            IpAddr::V6(address) => format!("[{address}]"),
        },
        ListenAddress::Unix(_) => "localhost".into(),
    });
    let default_port = if tls { 443 } else { 80 };
    let port = local
        .socket_addr()
        .map(|address| address.port())
        .filter(|port| *port != default_port)
        .filter(|_| context.port_in_redirect);
    let scheme = if tls { "https" } else { "http" };
    let mut target = format!("{scheme}://{host}");
    if let Some(port) = port {
        target.push(':');
        target.push_str(&port.to_string());
    }
    target.push_str(&location);
    response.set_header("Location", target);
}

pub(in crate::server) async fn apply_sub_filters(
    response: &mut Response,
    context: &EffectiveConfig,
    variables: &Variables,
) {
    if context.sub_filters.is_empty()
        || response.header("Content-Encoding").is_some()
        || response.body_len() == 0
    {
        return;
    }
    let content_type = response
        .header("Content-Type")
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim();
    if !context
        .sub_filter_types
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(content_type))
    {
        return;
    }
    let body = match std::mem::replace(&mut response.body, ResponseBody::Empty)
        .into_memory(MAX_BODY_BYTES as u64)
        .await
    {
        Ok(body) => body,
        Err(body) => {
            response.body = *body;
            return;
        }
    };
    let mut filtered = String::from_utf8_lossy(&body).into_owned();
    let mut changed = false;
    for filter in &context.sub_filters {
        let pattern = eval_variables(&filter.pattern, variables);
        if pattern.is_empty() {
            continue;
        }
        let replacement = eval_variables(&filter.replacement, variables);
        let next =
            replace_case_insensitive(&filtered, &pattern, &replacement, context.sub_filter_once);
        changed |= next != filtered;
        filtered = next;
    }
    if !changed {
        response.body = ResponseBody::Bytes(body);
        return;
    }
    if !context.sub_filter_last_modified {
        response.remove_header("Last-Modified");
        response.remove_header("ETag");
    }
    response.body = ResponseBody::Bytes(filtered.into_bytes());
    response.set_content_length();
}

pub(in crate::server) fn replace_case_insensitive(
    input: &str,
    pattern: &str,
    replacement: &str,
    once: bool,
) -> String {
    let lowercase = input.to_ascii_lowercase();
    let lowercase_pattern = pattern.to_ascii_lowercase();
    let mut output = String::with_capacity(input.len());
    let mut position = 0;
    while let Some(relative) = lowercase[position..].find(&lowercase_pattern) {
        let start = position + relative;
        output.push_str(&input[position..start]);
        output.push_str(replacement);
        position = start + pattern.len();
        if once {
            break;
        }
    }
    if position == 0 {
        return input.to_string();
    }
    output.push_str(&input[position..]);
    output
}
