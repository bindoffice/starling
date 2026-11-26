use super::*;

pub(super) fn automatic_https_action(
    config: &RuntimeConfig,
    group: &ListenerGroup,
    server: &ServerConfig,
    request: &Request,
    host: &str,
    state: &SharedState,
    http_error_loggers: &HttpErrorLoggers,
) -> Option<DispatchAction> {
    if group.ssl || request.method == "CONNECT" {
        return None;
    }
    let settings = &config.auto_https;
    if !settings.enabled {
        return None;
    }
    let (path, query) = request.path_and_query();
    if let Some(token) = path.strip_prefix(ACME_CHALLENGE_PREFIX) {
        let response = if !token.is_empty()
            && !token.contains('/')
            && let Some(key_authorization) = state.auto_https.challenge_response(token)
        {
            Response::bytes(200, "text/plain", key_authorization)
        } else {
            error_response(404)
        };
        return Some(DispatchAction::Response(builtin_auto_https_outcome(
            server,
            request,
            response,
            state,
            http_error_loggers,
        )));
    }
    if !settings.redirect_http
        || group.address.port() != settings.http_port
        || host.is_empty()
        || !server
            .listens
            .iter()
            .any(|listen| listen.ssl && listen.address.port() == settings.https_port)
    {
        return None;
    }
    let authority = https_authority(host, settings.https_port);
    let location = match query {
        Some(query) => format!("https://{authority}{path}?{query}"),
        None => format!("https://{authority}{path}"),
    };
    let mut response = Response::empty(308);
    response.set_header("Location", location);
    Some(DispatchAction::Response(builtin_auto_https_outcome(
        server,
        request,
        response,
        state,
        http_error_loggers,
    )))
}

fn https_authority(host: &str, port: u16) -> String {
    let hostname = if host.starts_with('[') {
        host.rsplit_once(']')
            .map(|(literal, _)| format!("{literal}]"))
            .unwrap_or_else(|| format!("[{host}]"))
    } else {
        host.split(':').next().unwrap_or(host).to_string()
    };
    if port == 443 {
        hostname
    } else {
        format!("{hostname}:{port}")
    }
}

fn builtin_auto_https_outcome(
    server: &ServerConfig,
    request: &Request,
    response: Response,
    state: &SharedState,
    http_error_loggers: &HttpErrorLoggers,
) -> DispatchOutcome {
    DispatchOutcome {
        response,
        response_prepared: false,
        response_chunked: false,
        error_logger: http_logger_for_context(state, http_error_loggers, &server.context),
        keepalive_timeout: server.context.keepalive_timeout,
        keepalive_min_timeout: server.context.keepalive_min_timeout,
        keep_alive: request.keep_alive,
        keepalive_header: None,
        send_timeout: server.context.send_timeout,
        tcp_nodelay: server.context.tcp_nodelay,
        limit_rate: ResponseRate::default(),
        server_tokens: server.context.server_tokens,
        chunked_transfer_encoding: server.context.chunked_transfer_encoding,
        sendfile: false,
        http2_chunk_size: server.context.http2_limits.chunk_size,
        close: ConnectionCloseConfig::from(&server.context),
        _body_file_guard: None,
        _connection_guard: None,
        deferred_access_log: None,
        upgrade: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_authority_replaces_or_omits_ports() {
        assert_eq!(https_authority("localhost:80", 443), "localhost");
        assert_eq!(https_authority("localhost:8080", 8443), "localhost:8443");
        assert_eq!(https_authority("[::1]:8080", 8443), "[::1]:8443");
        assert_eq!(https_authority("[::1]:80", 443), "[::1]");
    }
}
