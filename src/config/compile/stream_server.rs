use super::*;

use super::stream_pass::compile_stream_pass;
use super::stream_server_options::{
    StreamServerDefaults, parse_stream_bind, parse_stream_buffer_size, parse_stream_packet_count,
    parse_stream_rate, parse_stream_size,
};
use super::stream_tls_options::{StreamTlsOptions, StreamTlsSeen, apply_stream_tls_directive};

pub(super) fn compile_stream_server(
    directive: &Directive,
    upstreams: &HashMap<String, UpstreamConfig>,
    limit_conn_zones: &HashMap<String, Arc<LimitConnZone>>,
    id: usize,
    prefix: &Path,
    defaults: &StreamServerDefaults,
) -> ConfigResult<StreamServer> {
    let mut listens = Vec::new();
    let mut server_name = String::new();
    let mut names = Vec::new();
    let mut proxy_pass = None;
    let mut return_value = None;
    let mut pass = None;
    let mut handler = None;
    let mut sets = Vec::new();
    let mut proxy_timeout = Duration::from_secs(10 * 60);
    let mut proxy_connect_timeout = Duration::from_secs(60);
    let mut tls = defaults.tls.clone();
    let mut tls_seen = StreamTlsSeen::default();
    let mut access_log = defaults.access_log.clone();
    let mut access_log_seen = false;
    let mut open_log_file_cache = defaults.open_log_file_cache.clone();
    let mut open_log_file_cache_seen = false;
    let mut error_logs = defaults.error_logs.clone();
    let mut error_log_seen = false;
    let mut access_rules = defaults.access_rules.clone();
    let mut server_access_rules = false;
    let mut real_ip_from = defaults.real_ip_from.clone();
    let mut server_real_ip_from = false;
    let mut tcp_nodelay = defaults.tcp_nodelay;
    let mut ssl_preread = defaults.ssl_preread;
    let mut preread_timeout = defaults.preread_timeout;
    let mut preread_buffer_size = defaults.preread_buffer_size;
    let mut limit_conns = defaults.limit_conns.clone();
    let mut limit_conn_dry_run = defaults.limit_conn_dry_run;
    let mut limit_conn_log_level = defaults.limit_conn_log_level;
    let mut server_limit_conn = false;
    let mut proxy_protocol = defaults.proxy_protocol;
    let mut proxy_protocol_timeout = defaults.proxy_protocol_timeout;
    let mut proxy_half_close = defaults.proxy_half_close;
    let mut proxy_requests = defaults.proxy_requests;
    let mut proxy_responses = defaults.proxy_responses;
    let mut proxy_next_upstream = defaults.proxy_next_upstream;
    let mut proxy_next_upstream_tries = defaults.proxy_next_upstream_tries;
    let mut proxy_next_upstream_timeout = defaults.proxy_next_upstream_timeout;
    let mut proxy_upload_rate = defaults.proxy_upload_rate.clone();
    let mut proxy_download_rate = defaults.proxy_download_rate.clone();
    let mut proxy_buffer_size = defaults.proxy_buffer_size;
    let mut proxy_socket_keepalive = defaults.proxy_socket_keepalive;
    let mut proxy_socket_rcvbuf = defaults.proxy_socket_rcvbuf;
    let mut proxy_socket_sndbuf = defaults.proxy_socket_sndbuf;
    let mut proxy_bind = defaults.proxy_bind.clone();
    for child in &directive.children {
        if apply_stream_tls_directive(child, prefix, &mut tls, &mut tls_seen)? {
            continue;
        }
        match child.name.as_str() {
            "listen" => {
                validate_no_block(child)?;
                let parsed = parse_listen(child, prefix)?;
                for listen in parsed {
                    validate_stream_listen_options(&listen, &child.location)?;
                    validate_listen_conflicts(&listen, &listens, &child.location)?;
                    listens.push(listen);
                }
            }
            "server_name" => {
                validate_no_block(child)?;
                if child.args.is_empty() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "invalid number of arguments in \"server_name\" directive",
                    ));
                }
                for name in &child.args {
                    names.push(parse_server_name(name, &child.location)?);
                }
                if server_name.is_empty() {
                    let name = &child.args[0];
                    server_name = name.strip_prefix('.').unwrap_or(name).to_ascii_lowercase();
                }
            }
            "proxy_pass" => {
                validate_no_block(child)?;
                let target = one_arg(child)?.to_string();
                let upstream = if upstreams.contains_key(&target) {
                    Some(target.clone())
                } else {
                    None
                };
                proxy_pass = Some(StreamProxyPass { target, upstream });
                handler = Some(StreamHandler::Proxy);
            }
            "return" => {
                validate_no_block(child)?;
                if return_value.is_some() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"return\" directive is duplicate",
                    ));
                }
                return_value = Some(one_arg(child)?.to_string());
                handler = Some(StreamHandler::Return);
            }
            "pass" => {
                if pass.is_some() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"pass\" directive is duplicate",
                    ));
                }
                pass = Some(compile_stream_pass(child, prefix)?);
                handler = Some(StreamHandler::Pass);
            }
            "proxy_timeout" => {
                validate_no_block(child)?;
                proxy_timeout = parse_duration(one_arg(child)?, &child.location)?;
            }
            "proxy_connect_timeout" => {
                validate_no_block(child)?;
                proxy_connect_timeout = parse_duration(one_arg(child)?, &child.location)?;
            }
            "access_log" => apply_access_log_directive(
                child,
                prefix,
                None,
                &mut access_log,
                &mut access_log_seen,
            )?,
            "open_log_file_cache" => {
                try_apply_open_log_file_cache_directive(
                    child,
                    &mut open_log_file_cache,
                    &mut open_log_file_cache_seen,
                )?;
            }
            "error_log" => {
                if !error_log_seen {
                    error_logs.clear();
                    error_log_seen = true;
                }
                error_logs.push(parse_log_target(prefix, child)?);
            }
            "allow" | "deny" => {
                validate_no_block(child)?;
                if !server_access_rules {
                    access_rules.clear();
                    server_access_rules = true;
                }
                access_rules.push(parse_access_rule(child)?);
            }
            "set_real_ip_from" => {
                validate_no_block(child)?;
                let value = one_arg(child)?;
                if !server_real_ip_from {
                    real_ip_from.clear();
                    server_real_ip_from = true;
                }
                real_ip_from.push(parse_real_ip_source(value, &child.location)?);
            }
            "tcp_nodelay" => tcp_nodelay = parse_flag(child, "tcp_nodelay")?,
            "ssl_preread" => ssl_preread = parse_flag(child, "ssl_preread")?,
            "preread_timeout" => {
                validate_no_block(child)?;
                preread_timeout = parse_duration(one_arg(child)?, &child.location)?;
            }
            "preread_buffer_size" => {
                preread_buffer_size = parse_stream_buffer_size(child, "preread_buffer_size")?;
            }
            "set" => {
                validate_no_block(child)?;
                sets.push(parse_set(child)?);
            }
            "limit_conn" => {
                validate_no_block(child)?;
                if !server_limit_conn {
                    limit_conns.clear();
                    server_limit_conn = true;
                }
                let limit = compile_stream_limit_conn(child, limit_conn_zones)?;
                if limit_conns
                    .iter()
                    .any(|existing| existing.zone == limit.zone)
                {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"limit_conn\" directive is duplicate",
                    ));
                }
                limit_conns.push(limit);
            }
            "limit_conn_dry_run" => {
                limit_conn_dry_run = parse_flag(child, "limit_conn_dry_run")?;
            }
            "limit_conn_log_level" => {
                limit_conn_log_level = parse_limit_log_level(child, "limit_conn_log_level")?;
            }
            "proxy_protocol" => proxy_protocol = parse_stream_proxy_protocol(child)?,
            "proxy_protocol_timeout" => {
                validate_no_block(child)?;
                proxy_protocol_timeout = parse_duration(one_arg(child)?, &child.location)?;
            }
            "proxy_half_close" => {
                proxy_half_close = parse_flag(child, "proxy_half_close")?;
            }
            "proxy_requests" => {
                proxy_requests = parse_stream_packet_count(child, "proxy_requests")?;
            }
            "proxy_responses" => {
                proxy_responses = parse_stream_packet_count(child, "proxy_responses")?;
            }
            "proxy_next_upstream" => {
                proxy_next_upstream = parse_flag(child, "proxy_next_upstream")?;
            }
            "proxy_next_upstream_tries" => {
                proxy_next_upstream_tries =
                    parse_stream_packet_count(child, "proxy_next_upstream_tries")?;
            }
            "proxy_next_upstream_timeout" => {
                validate_no_block(child)?;
                proxy_next_upstream_timeout = parse_duration(one_arg(child)?, &child.location)?;
            }
            "proxy_upload_rate" => {
                proxy_upload_rate = parse_stream_rate(child, "proxy_upload_rate")?;
            }
            "proxy_download_rate" => {
                proxy_download_rate = parse_stream_rate(child, "proxy_download_rate")?;
            }
            "proxy_buffer_size" | "proxy_downstream_buffer" | "proxy_upstream_buffer" => {
                proxy_buffer_size = parse_stream_buffer_size(child, &child.name)?;
            }
            "proxy_socket_keepalive" => {
                proxy_socket_keepalive = parse_flag(child, "proxy_socket_keepalive")?;
            }
            "proxy_socket_rcvbuf" => {
                proxy_socket_rcvbuf = parse_stream_size(child, "proxy_socket_rcvbuf")?;
            }
            "proxy_socket_sndbuf" => {
                proxy_socket_sndbuf = parse_stream_size(child, "proxy_socket_sndbuf")?;
            }
            "proxy_bind" => {
                proxy_bind = parse_stream_bind(child)?;
            }
            "if" => {
                return Err(ConfigError::at(
                    child.location.clone(),
                    "\"if\" is not supported in stream server context",
                ));
            }
            other => {
                return Err(ConfigError::at(
                    child.location.clone(),
                    format!("unknown directive \"{other}\" in stream server context"),
                ));
            }
        }
    }
    if listens.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "no \"listen\" is defined for stream server",
        ));
    }
    if handler.is_none() {
        handler = Some(StreamHandler::Return);
        return_value = Some(String::new());
    }
    // Nginx only starts upstream TLS for SOCK_STREAM connections.  It still
    // accepts proxy_ssl for UDP listen sockets, where the setting is inert.
    validate_ssl_client_verification(
        &directive.location,
        tls.ssl_verify_client,
        tls.ssl_client_certificate.as_ref(),
        tls.ssl_trusted_certificate.as_ref(),
    )?;
    if tls.ssl_stapling && tls.ssl_stapling_verify && tls.ssl_trusted_certificate.is_none() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "no ssl_trusted_certificate for ssl_stapling_verify",
        ));
    }
    if tls.ssl_ocsp != OcspMode::Off
        && tls.ssl_verify_client == ClientCertificateVerify::OptionalNoCa
    {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"ssl_ocsp\" is incompatible with \"ssl_verify_client optional_no_ca\"",
        ));
    }
    let StreamTlsOptions {
        proxy_ssl,
        proxy_ssl_protocols,
        proxy_ssl_ciphers,
        proxy_ssl_server_name,
        proxy_ssl_name,
        proxy_ssl_session_reuse,
        proxy_ssl_verify,
        proxy_ssl_trusted_certificate,
        proxy_ssl_verify_depth,
        proxy_ssl_crl,
        proxy_ssl_certificate,
        proxy_ssl_certificate_key,
        proxy_ssl_passwords,
        proxy_ssl_alpn,
        ssl_certificate,
        ssl_certificate_key,
        ssl_certificates,
        ssl_certificate_keys,
        ssl_protocols,
        ssl_prefer_server_ciphers,
        ssl_ciphers,
        ssl_verify_client,
        ssl_verify_depth,
        ssl_client_certificate,
        ssl_trusted_certificate,
        ssl_crl,
        ssl_stapling,
        ssl_stapling_verify,
        ssl_stapling_file,
        ssl_stapling_responder,
        ssl_ocsp,
        ssl_ocsp_responder,
        ssl_ocsp_cache,
        resolver,
        resolver_timeout,
        ssl_passwords,
        ssl_alpn,
        ssl_handshake_timeout,
        ssl_reject_handshake,
    } = tls;
    Ok(StreamServer {
        id,
        listens,
        server_name,
        names,
        error_logs,
        access_log,
        open_log_file_cache,
        access_rules,
        real_ip_from,
        tcp_nodelay,
        ssl_preread,
        preread_timeout,
        preread_buffer_size,
        proxy_pass,
        return_value,
        pass,
        handler: handler.expect("stream handler is set when a handler directive exists"),
        sets,
        limit_conns,
        limit_conn_dry_run,
        limit_conn_log_level,
        proxy_protocol,
        proxy_protocol_timeout,
        proxy_half_close,
        proxy_timeout,
        proxy_connect_timeout,
        proxy_requests,
        proxy_responses,
        proxy_next_upstream,
        proxy_next_upstream_tries,
        proxy_next_upstream_timeout,
        proxy_upload_rate,
        proxy_download_rate,
        proxy_buffer_size,
        proxy_socket_keepalive,
        proxy_socket_rcvbuf,
        proxy_socket_sndbuf,
        proxy_bind,
        proxy_ssl,
        proxy_ssl_server_name,
        proxy_ssl_name,
        proxy_ssl_session_reuse,
        proxy_ssl_verify,
        proxy_ssl_trusted_certificate,
        proxy_ssl_verify_depth,
        proxy_ssl_protocols,
        ssl_certificate,
        ssl_certificate_key,
        ssl_certificates,
        ssl_certificate_keys,
        ssl_verify_client,
        ssl_verify_depth,
        ssl_client_certificate,
        ssl_trusted_certificate,
        ssl_crl,
        ssl_stapling,
        ssl_stapling_verify,
        ssl_stapling_file,
        ssl_stapling_responder,
        ssl_ocsp,
        ssl_ocsp_responder,
        ssl_ocsp_cache,
        resolver,
        resolver_timeout,
        proxy_ssl_crl,
        proxy_ssl_certificate,
        proxy_ssl_certificate_key,
        proxy_ssl_passwords,
        proxy_ssl_alpn,
        ssl_protocols,
        ssl_prefer_server_ciphers,
        ssl_ciphers,
        proxy_ssl_ciphers,
        ssl_alpn,
        ssl_passwords,
        ssl_handshake_timeout,
        ssl_reject_handshake,
    })
}
