use super::*;

pub(in crate::config::compile) fn parse_listen(
    directive: &Directive,
    prefix: &Path,
) -> ConfigResult<Vec<ListenConfig>> {
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"listen\" directive",
        ));
    }
    let value = &directive.args[0];
    let addresses = if let Some(path) = value.strip_prefix("unix:") {
        if path.is_empty() {
            return Err(ConfigError::at(
                directive.location.clone(),
                "invalid empty UNIX domain listen path",
            ));
        }
        let path = resolve_path(prefix, path);
        if path.to_string_lossy().len() >= 108 {
            return Err(ConfigError::at(
                directive.location.clone(),
                "UNIX domain listen path is too long",
            ));
        }
        vec![ListenAddress::Unix(path)]
    } else {
        parse_listen_addresses(value, &directive.location)?
            .into_iter()
            .map(ListenAddress::Inet)
            .collect()
    };
    let mut listen = ListenConfig {
        address: addresses[0].clone(),
        ssl: false,
        http2: false,
        quic: false,
        udp: false,
        proxy_protocol: false,
        default_server: false,
        socket_options: ListenSocketOptions::default(),
        socket_options_set: false,
    };
    for option in &directive.args[1..] {
        match option.as_str() {
            "ssl" => listen.ssl = true,
            "http2" => listen.http2 = true,
            "quic" => listen.quic = true,
            "udp" => listen.udp = true,
            "proxy_protocol" => listen.proxy_protocol = true,
            "default_server" | "default" => listen.default_server = true,
            "bind" => {
                listen.socket_options.bind = true;
                listen.socket_options_set = true;
            }
            "reuseport" => {
                listen.socket_options.reuseport = true;
                listen.socket_options_set = true;
            }
            "multipath" if cfg!(target_os = "linux") => {
                listen.socket_options.multipath = true;
                listen.socket_options_set = true;
            }
            "deferred" => {
                listen.socket_options.deferred = true;
                listen.socket_options_set = true;
            }
            _ if option.starts_with("backlog=") => {
                let value = &option["backlog=".len()..];
                let backlog = parse_listen_u32(value, &directive.location, "backlog")?;
                if backlog == 0 || backlog > i32::MAX as u32 {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!("invalid backlog \"{option}\""),
                    ));
                }
                listen.socket_options.backlog = backlog;
                listen.socket_options.backlog_set = true;
                listen.socket_options_set = true;
            }
            _ if option.starts_with("rcvbuf=") => {
                listen.socket_options.receive_buffer_size =
                    Some(parse_listen_size(option, "rcvbuf", &directive.location)?);
                listen.socket_options_set = true;
            }
            _ if option.starts_with("sndbuf=") => {
                listen.socket_options.send_buffer_size =
                    Some(parse_listen_size(option, "sndbuf", &directive.location)?);
                listen.socket_options_set = true;
            }
            _ if option.starts_with("ipv6only=") => {
                listen.socket_options.ipv6only = match &option["ipv6only=".len()..] {
                    "on" => true,
                    "off" => false,
                    flags => {
                        return Err(ConfigError::at(
                            directive.location.clone(),
                            format!("invalid ipv6only flags \"{flags}\""),
                        ));
                    }
                };
                listen.socket_options_set = true;
            }
            _ if option.starts_with("so_keepalive=") => {
                listen.socket_options.keepalive =
                    parse_listen_keepalive(&option["so_keepalive=".len()..], &directive.location)?;
                listen.socket_options_set = true;
            }
            _ if option.starts_with("fastopen=") => {
                let fastopen = parse_listen_u32(
                    &option["fastopen=".len()..],
                    &directive.location,
                    "fastopen",
                )?;
                if fastopen > i32::MAX as u32 {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!("invalid fastopen \"{option}\""),
                    ));
                }
                listen.socket_options.fastopen = Some(fastopen);
                listen.socket_options_set = true;
            }
            _ if option.starts_with("accept_filter=") => {
                // Nginx accepts this parameter on platforms without SO_ACCEPTFILTER
                // and logs the limitation without rejecting the configuration.
            }
            _ if option.starts_with("setfib=") => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid parameter \"{option}\" in listen directive"),
                ));
            }
            other => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid parameter \"{other}\" in listen directive"),
                ));
            }
        }
    }
    Ok(addresses
        .into_iter()
        .map(|address| ListenConfig {
            address,
            ..listen.clone()
        })
        .collect())
}

pub(in crate::config::compile) fn validate_stream_listen_options(
    listen: &ListenConfig,
    location: &SourceLocation,
) -> ConfigResult<()> {
    if listen.http2 {
        return Err(ConfigError::at(
            location.clone(),
            "invalid parameter \"http2\" in listen directive",
        ));
    }
    if listen.quic {
        return Err(ConfigError::at(
            location.clone(),
            "invalid parameter \"quic\" in listen directive",
        ));
    }
    if !listen.udp {
        return Ok(());
    }
    if listen.address.unix_path().is_some() {
        return Err(ConfigError::at(
            location.clone(),
            "\"udp\" is incompatible with UNIX domain listen sockets",
        ));
    }
    for (enabled, parameter) in [
        (listen.ssl, "ssl"),
        (listen.proxy_protocol, "proxy_protocol"),
        (listen.socket_options.fastopen.is_some(), "fastopen"),
        (listen.socket_options.backlog_set, "backlog"),
        (listen.socket_options.deferred, "deferred"),
        (listen.socket_options.multipath, "multipath"),
        (
            listen.socket_options.keepalive != ListenKeepalive::Unset,
            "so_keepalive",
        ),
    ] {
        if enabled {
            return Err(ConfigError::at(
                location.clone(),
                format!("\"{parameter}\" parameter is incompatible with \"udp\""),
            ));
        }
    }
    Ok(())
}

pub(in crate::config::compile) fn validate_listen_conflicts(
    listen: &ListenConfig,
    existing: &[ListenConfig],
    location: &SourceLocation,
) -> ConfigResult<()> {
    for previous in existing {
        check_listen_conflict(listen, previous, location)?;
    }
    Ok(())
}

pub(in crate::config::compile) fn validate_all_listen_conflicts<'a>(
    listens: impl IntoIterator<Item = &'a ListenConfig>,
    location: &SourceLocation,
) -> ConfigResult<()> {
    let mut seen: Vec<&ListenConfig> = Vec::new();
    for listen in listens {
        for previous in seen.iter().copied() {
            check_listen_conflict(listen, previous, location)?;
        }
        seen.push(listen);
    }
    Ok(())
}

fn check_listen_conflict(
    listen: &ListenConfig,
    previous: &ListenConfig,
    location: &SourceLocation,
) -> ConfigResult<()> {
    if previous.address != listen.address || previous.udp != listen.udp {
        return Ok(());
    }
    if listen.socket_options_set && previous.socket_options_set {
        return Err(ConfigError::at(
            location.clone(),
            format!("duplicate listen options for {}", listen.address),
        ));
    }
    if listen.default_server && previous.default_server {
        return Err(ConfigError::at(
            location.clone(),
            format!("a duplicate default server for {}", listen.address),
        ));
    }
    Ok(())
}

fn parse_listen_u32(value: &str, location: &SourceLocation, name: &str) -> ConfigResult<u32> {
    value.parse::<u32>().map_err(|_| {
        ConfigError::at(
            location.clone(),
            format!("invalid {name} \"{name}={value}\""),
        )
    })
}

fn parse_listen_size(option: &str, name: &str, location: &SourceLocation) -> ConfigResult<usize> {
    let value = &option[name.len() + 1..];
    let size = parse_size(value, location)
        .map_err(|_| ConfigError::at(location.clone(), format!("invalid {name} \"{option}\"")))?;
    usize::try_from(size)
        .ok()
        .filter(|size| *size <= i32::MAX as usize)
        .ok_or_else(|| ConfigError::at(location.clone(), format!("invalid {name} \"{option}\"")))
}

fn parse_listen_keepalive(value: &str, location: &SourceLocation) -> ConfigResult<ListenKeepalive> {
    match value {
        "on" => return Ok(ListenKeepalive::On),
        "off" => return Ok(ListenKeepalive::Off),
        _ => {}
    }

    let mut fields = value.splitn(4, ':');
    let idle = parse_keepalive_time(fields.next().unwrap_or_default(), location)?;
    let interval = parse_keepalive_time(fields.next().unwrap_or_default(), location)?;
    let count = fields
        .next()
        .filter(|field| !field.is_empty())
        .map(|field| {
            field.parse::<u32>().map_err(|_| {
                ConfigError::at(
                    location.clone(),
                    format!("invalid so_keepalive value: \"{value}\""),
                )
            })
        })
        .transpose()?;
    if fields.next().is_some()
        || (idle.is_none_or(|time| time.is_zero())
            && interval.is_none_or(|time| time.is_zero())
            && count.is_none_or(|count| count == 0))
    {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid so_keepalive value: \"{value}\""),
        ));
    }
    Ok(ListenKeepalive::Tuned {
        idle,
        interval,
        count,
    })
}

fn parse_keepalive_time(value: &str, location: &SourceLocation) -> ConfigResult<Option<Duration>> {
    if value.is_empty() {
        return Ok(None);
    }
    let seconds = parse_time_seconds(value, location).map_err(|_| {
        ConfigError::at(
            location.clone(),
            format!("invalid so_keepalive value: \"{value}\""),
        )
    })?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid so_keepalive value: \"{value}\""),
        ));
    }
    Duration::try_from_secs_f64(seconds).map(Some).map_err(|_| {
        ConfigError::at(
            location.clone(),
            format!("invalid so_keepalive value: \"{value}\""),
        )
    })
}

pub(super) fn parse_listen_address(
    value: &str,
    location: &SourceLocation,
) -> ConfigResult<SocketAddr> {
    if let Ok(port) = value.parse::<u16>() {
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port));
    }
    if let Some(port) = value.strip_prefix("*:") {
        return Ok(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            parse_port(port, location)?,
        ));
    }
    if let Ok(address) = value.parse::<SocketAddr>() {
        return Ok(address);
    }
    if let Ok(ip) = value.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, 80));
    }
    if let Some((host, port)) = value.rsplit_once(':') {
        let port = parse_port(port, location)?;
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(SocketAddr::new(ip, port));
        }
        let resolved = (host, port)
            .to_socket_addrs()
            .map_err(|err| {
                ConfigError::at(
                    location.clone(),
                    format!("host not found in \"{value}\": {err}"),
                )
            })?
            .next()
            .ok_or_else(|| {
                ConfigError::at(location.clone(), format!("host not found in \"{value}\""))
            })?;
        return Ok(resolved);
    }
    let resolved = (value, 80)
        .to_socket_addrs()
        .map_err(|err| {
            ConfigError::at(
                location.clone(),
                format!("host not found in \"{value}\": {err}"),
            )
        })?
        .next()
        .ok_or_else(|| {
            ConfigError::at(location.clone(), format!("host not found in \"{value}\""))
        })?;
    Ok(resolved)
}

fn parse_listen_addresses(value: &str, location: &SourceLocation) -> ConfigResult<Vec<SocketAddr>> {
    let Some((host, ports)) = value
        .rsplit_once(':')
        .or_else(|| (!value.contains(':')).then_some(("", value)))
    else {
        return Ok(vec![parse_listen_address(value, location)?]);
    };
    if !ports.contains('-') {
        return Ok(vec![parse_listen_address(value, location)?]);
    }
    let (start, end) = ports.split_once('-').ok_or_else(|| {
        ConfigError::at(
            location.clone(),
            format!("invalid port range in listen address: {ports}"),
        )
    })?;
    let start = parse_port(start, location)?;
    let end = parse_port(end, location)?;
    if start == 0 || start > end {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid port range in listen address: {ports}"),
        ));
    }
    let seed = if host.is_empty() {
        start.to_string()
    } else {
        format!("{host}:{start}")
    };
    let seed = parse_listen_address(&seed, location)?;
    Ok((start..=end)
        .map(|port| SocketAddr::new(seed.ip(), port))
        .collect())
}

pub(in crate::config::compile) fn parse_port(
    value: &str,
    location: &SourceLocation,
) -> ConfigResult<u16> {
    value.parse::<u16>().map_err(|_| {
        ConfigError::at(
            location.clone(),
            format!("invalid port in listen address: {value}"),
        )
    })
}
