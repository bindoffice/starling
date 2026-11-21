use super::*;

use super::stream_server::compile_stream_server;
use super::stream_server_options::{
    DEFAULT_STREAM_PREREAD_BUFFER_SIZE, DEFAULT_STREAM_PROXY_BUFFER_SIZE, StreamServerDefaults,
    parse_stream_bind, parse_stream_buffer_size, parse_stream_packet_count, parse_stream_rate,
    parse_stream_size,
};
use super::stream_tls_options::{StreamTlsOptions, StreamTlsSeen, apply_stream_tls_directive};

pub(super) fn compile_stream(directive: &Directive, prefix: &Path) -> ConfigResult<StreamConfig> {
    let mut upstreams = HashMap::new();
    let mut server_directives = Vec::new();
    let mut log_formats = HashMap::new();
    let mut maps = HashMap::new();
    let mut geos = HashMap::new();
    let mut split_clients = HashMap::new();
    let mut limit_conn_zones = HashMap::new();
    let mut limit_conn_directives = Vec::new();
    let mut limit_conn_dry_run = false;
    let mut limit_conn_log_level = LogLevel::Error;
    let mut error_logs = Vec::new();
    let mut proxy_protocol = StreamProxyProtocol::Off;
    let mut proxy_protocol_timeout = Duration::from_secs(30);
    let mut proxy_half_close = false;
    let mut proxy_requests = 0;
    let mut proxy_responses = STREAM_PROXY_RESPONSES_UNLIMITED;
    let mut proxy_next_upstream = true;
    let mut proxy_next_upstream_tries = 0;
    let mut proxy_next_upstream_timeout = Duration::ZERO;
    let mut proxy_upload_rate = None;
    let mut proxy_download_rate = None;
    let mut proxy_buffer_size = DEFAULT_STREAM_PROXY_BUFFER_SIZE;
    let mut proxy_socket_keepalive = false;
    let mut proxy_socket_rcvbuf = 0;
    let mut proxy_socket_sndbuf = 0;
    let mut proxy_bind = None;
    let mut access_log = AccessLogConfig::default();
    let mut access_log_seen = false;
    let mut open_log_file_cache = None;
    let mut open_log_file_cache_seen = false;
    let mut access_rules = Vec::new();
    let mut real_ip_from = Vec::new();
    let mut seen_real_ip_from = false;
    let mut tcp_nodelay = true;
    let mut ssl_preread = false;
    let mut preread_timeout = Duration::from_secs(30);
    let mut preread_buffer_size = DEFAULT_STREAM_PREREAD_BUFFER_SIZE;
    let mut tls = StreamTlsOptions::default();
    let mut tls_seen = StreamTlsSeen::default();
    for child in &directive.children {
        if apply_stream_tls_directive(child, prefix, &mut tls, &mut tls_seen)? {
            continue;
        }
        match child.name.as_str() {
            "upstream" => {
                require_block(child)?;
                let upstream = compile_upstream(child, UpstreamContext::Stream)?;
                if upstreams.insert(upstream.name.clone(), upstream).is_some() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate upstream",
                    ));
                }
            }
            "map" => {
                require_block(child)?;
                let map = compile_map(child)?;
                if geos.contains_key(&map.name)
                    || split_clients.contains_key(&map.name)
                    || maps.insert(map.name.clone(), Arc::new(map)).is_some()
                {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate map variable",
                    ));
                }
            }
            "geo" => {
                require_block(child)?;
                let geo = compile_geo(child)?;
                if maps.contains_key(&geo.name)
                    || split_clients.contains_key(&geo.name)
                    || geos.insert(geo.name.clone(), Arc::new(geo)).is_some()
                {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate geo variable",
                    ));
                }
            }
            "split_clients" => {
                require_block(child)?;
                let split = compile_split_clients(child)?;
                if maps.contains_key(&split.name)
                    || geos.contains_key(&split.name)
                    || split_clients
                        .insert(split.name.clone(), Arc::new(split))
                        .is_some()
                {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate split_clients variable",
                    ));
                }
            }
            "server" => {
                require_block(child)?;
                server_directives.push(child);
            }
            "limit_conn_zone" => {
                validate_no_block(child)?;
                let zone = compile_limit_conn_zone(child)?;
                if limit_conn_zones
                    .insert(zone.name.clone(), Arc::new(zone))
                    .is_some()
                {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate limit_conn_zone",
                    ));
                }
            }
            "limit_conn" => limit_conn_directives.push(child),
            "limit_conn_dry_run" => {
                limit_conn_dry_run = parse_flag(child, "limit_conn_dry_run")?;
            }
            "limit_conn_log_level" => {
                limit_conn_log_level = parse_limit_log_level(child, "limit_conn_log_level")?;
            }
            "error_log" => error_logs.push(parse_log_target(prefix, child)?),
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
            "log_format" => compile_log_format(child, &mut log_formats, false)?,
            "access_log" => {
                apply_access_log_directive(
                    child,
                    prefix,
                    None,
                    &mut access_log,
                    &mut access_log_seen,
                )?;
            }
            "open_log_file_cache" => {
                try_apply_open_log_file_cache_directive(
                    child,
                    &mut open_log_file_cache,
                    &mut open_log_file_cache_seen,
                )?;
            }
            "allow" | "deny" => {
                validate_no_block(child)?;
                access_rules.push(parse_access_rule(child)?);
            }
            "set_real_ip_from" => {
                validate_no_block(child)?;
                let value = one_arg(child)?;
                if !seen_real_ip_from {
                    real_ip_from.clear();
                    seen_real_ip_from = true;
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
            "resolver"
            | "resolver_timeout"
            | "ssl_stapling"
            | "ssl_stapling_verify"
            | "ssl_stapling_file"
            | "ssl_stapling_responder"
            | "variables_hash_max_size"
            | "variables_hash_bucket_size"
            | "map_hash_max_size"
            | "map_hash_bucket_size" => {
                apply_stream_tls_directive(child, prefix, &mut tls, &mut tls_seen)?;
            }
            other => {
                return Err(ConfigError::at(
                    child.location.clone(),
                    format!("unknown directive \"{other}\" in stream context"),
                ));
            }
        }
    }
    let server_defaults = StreamServerDefaults {
        error_logs,
        tls,
        access_log,
        open_log_file_cache,
        access_rules,
        real_ip_from,
        tcp_nodelay,
        ssl_preread,
        preread_timeout,
        preread_buffer_size,
        limit_conns: compile_stream_limit_conns(&limit_conn_directives, &limit_conn_zones)?,
        limit_conn_dry_run,
        limit_conn_log_level,
        proxy_protocol,
        proxy_protocol_timeout,
        proxy_half_close,
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
    };
    let mut servers = Vec::with_capacity(server_directives.len());
    for (id, server_directive) in server_directives.into_iter().enumerate() {
        servers.push(compile_stream_server(
            server_directive,
            &upstreams,
            &limit_conn_zones,
            id,
            prefix,
            &server_defaults,
        )?);
    }
    validate_all_listen_conflicts(
        servers.iter().flat_map(|server| &server.listens),
        &directive.location,
    )?;
    for server in &servers {
        validate_access_log_formats(&server.access_log, &log_formats, false, &directive.location)?;
    }
    Ok(StreamConfig {
        servers,
        error_logs: server_defaults.error_logs,
        upstreams,
        maps: Arc::new(maps),
        geos: Arc::new(geos),
        split_clients: Arc::new(split_clients),
        limit_conn_zones: Arc::new(limit_conn_zones),
        log_formats,
    })
}

fn compile_stream_limit_conns(
    directives: &[&Directive],
    zones: &HashMap<String, Arc<LimitConnZone>>,
) -> ConfigResult<Vec<LimitConnLimit>> {
    let mut limits = Vec::new();
    for directive in directives {
        let limit = compile_stream_limit_conn(directive, zones)?;
        if limits
            .iter()
            .any(|existing: &LimitConnLimit| existing.zone == limit.zone)
        {
            return Err(ConfigError::at(
                directive.location.clone(),
                "\"limit_conn\" directive is duplicate",
            ));
        }
        limits.push(limit);
    }
    Ok(limits)
}

pub(super) fn compile_types(
    directive: &Directive,
    types: &mut HashMap<String, String>,
) -> ConfigResult<()> {
    for entry in &directive.children {
        if entry.has_block || entry.args.is_empty() {
            return Err(ConfigError::at(
                entry.location.clone(),
                "invalid type declaration",
            ));
        }
        for extension in &entry.args {
            types.insert(extension.to_ascii_lowercase(), entry.name.clone());
        }
    }
    Ok(())
}

pub(super) fn compile_log_format(
    directive: &Directive,
    formats: &mut HashMap<String, AccessLogFormat>,
    reserve_combined: bool,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if directive.args.len() < 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"log_format\" directive",
        ));
    }
    let name = directive.args[0].clone();
    let mut start = 1;
    let mut log_escape = LogEscapeMode::Default;
    if let Some(escape) = directive.args.get(1)
        && let Some(escape) = escape.strip_prefix("escape=")
    {
        log_escape = match escape {
            "default" => LogEscapeMode::Default,
            "json" => LogEscapeMode::Json,
            "none" => LogEscapeMode::None,
            _ => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid escape parameter \"{}\"", directive.args[1]),
                ));
            }
        };
        start = 2;
    }
    if start == directive.args.len() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"log_format\" directive",
        ));
    }
    if formats.contains_key(&name) || (reserve_combined && name == "combined") {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("duplicate \"log_format\" name \"{name}\""),
        ));
    }
    let value = directive.args[start..].join("");
    formats.insert(
        name,
        AccessLogFormat {
            template: value,
            escape: log_escape,
        },
    );
    Ok(())
}
