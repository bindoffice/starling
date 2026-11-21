use super::*;

pub(in crate::config::compile) fn compile_limit_req_zone(
    directive: &Directive,
) -> ConfigResult<LimitReqZone> {
    if directive.args.len() != 3 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"limit_req_zone\" directive",
        ));
    }
    let key = directive.args[0].clone();
    let mut name = None;
    let mut rate_millis = None;
    for parameter in &directive.args[1..] {
        if let Some(zone) = parameter.strip_prefix("zone=") {
            let Some((zone_name, size)) = zone.split_once(':') else {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid zone size \"{zone}\""),
                ));
            };
            if zone_name.is_empty() {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid zone size \"{zone}\""),
                ));
            }
            parse_size(size, &directive.location)?;
            name = Some(zone_name.to_string());
        } else if let Some(rate) = parameter.strip_prefix("rate=") {
            rate_millis = Some(parse_limit_rate(rate, &directive.location)?);
        } else {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("invalid parameter \"{parameter}\""),
            ));
        }
    }
    let Some(name) = name else {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"limit_req_zone\" must have \"zone\" parameter",
        ));
    };
    let Some(rate_millis) = rate_millis else {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"limit_req_zone\" must have \"rate\" parameter",
        ));
    };
    Ok(LimitReqZone {
        name,
        key,
        rate_millis,
    })
}

pub(in crate::config::compile) fn compile_limit_conn_zone(
    directive: &Directive,
) -> ConfigResult<LimitConnZone> {
    if directive.args.len() != 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"limit_conn_zone\" directive",
        ));
    }
    let key = directive.args[0].clone();
    let Some(zone) = directive.args[1].strip_prefix("zone=") else {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"limit_conn_zone\" must have \"zone\" parameter",
        ));
    };
    let Some((name, size)) = zone.split_once(':') else {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid zone size \"{zone}\""),
        ));
    };
    if name.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid zone size \"{zone}\""),
        ));
    }
    parse_size(size, &directive.location)?;
    Ok(LimitConnZone {
        name: name.to_string(),
        key,
    })
}

pub(in crate::config::compile) fn compile_limit_req(
    directive: &Directive,
    zones: &HashMap<String, Arc<LimitReqZone>>,
) -> ConfigResult<LimitReqLimit> {
    if directive.args.is_empty() || directive.args.len() > 3 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"limit_req\" directive",
        ));
    }
    let mut zone = None;
    let mut burst = 0;
    let mut delay = 0;
    let mut nodelay = false;
    for parameter in &directive.args {
        if let Some(name) = parameter.strip_prefix("zone=") {
            zone = Some(name.to_string());
        } else if let Some(value) = parameter.strip_prefix("burst=") {
            burst = value.parse::<u64>().map_err(|_| {
                ConfigError::at(
                    directive.location.clone(),
                    format!("invalid burst value \"{value}\""),
                )
            })?;
            if burst == 0 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid burst value \"{value}\""),
                ));
            }
        } else if let Some(value) = parameter.strip_prefix("delay=") {
            delay = value.parse::<u64>().map_err(|_| {
                ConfigError::at(
                    directive.location.clone(),
                    format!("invalid delay value \"{value}\""),
                )
            })?;
            if delay == 0 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid delay value \"{value}\""),
                ));
            }
        } else if parameter == "nodelay" {
            nodelay = true;
        } else {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("invalid parameter \"{parameter}\""),
            ));
        }
    }
    let Some(zone_name) = zone else {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"limit_req\" must have \"zone\" parameter",
        ));
    };
    let Some(zone) = zones.get(&zone_name) else {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("unknown limit_req_zone \"{zone_name}\""),
        ));
    };
    Ok(LimitReqLimit {
        zone: zone_name,
        key: zone.key.clone(),
        rate_millis: zone.rate_millis,
        burst,
        delay,
        nodelay,
    })
}

pub(in crate::config::compile) fn compile_limit_conn(
    directive: &Directive,
    zones: &HashMap<String, Arc<LimitConnZone>>,
) -> ConfigResult<LimitConnLimit> {
    if directive.args.len() != 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"limit_conn\" directive",
        ));
    }
    let zone_name = &directive.args[0];
    let Some(zone) = zones.get(zone_name) else {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("unknown limit_conn_zone \"{zone_name}\""),
        ));
    };
    let connections = parse_connection_limit(&directive.args[1], directive)?;
    Ok(LimitConnLimit {
        zone: zone_name.clone(),
        key: zone.key.clone(),
        connections,
    })
}

pub(in crate::config::compile) fn compile_stream_limit_conn(
    directive: &Directive,
    zones: &HashMap<String, Arc<LimitConnZone>>,
) -> ConfigResult<LimitConnLimit> {
    if directive.args.len() != 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"limit_conn\" directive",
        ));
    }
    let zone_name = &directive.args[0];
    let Some(zone) = zones.get(zone_name) else {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("unknown limit_conn_zone \"{zone_name}\""),
        ));
    };
    Ok(LimitConnLimit {
        zone: zone_name.clone(),
        key: zone.key.clone(),
        connections: parse_connection_limit(&directive.args[1], directive)?,
    })
}

fn parse_connection_limit(value: &str, directive: &Directive) -> ConfigResult<u64> {
    let connections = value.parse::<u64>().map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid connection limit \"{value}\""),
        )
    })?;
    if connections == 0 {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid connection limit \"{value}\""),
        ));
    }
    if connections > 65535 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "connection limit must be less 65536",
        ));
    }
    Ok(connections)
}

pub(in crate::config::compile) fn parse_limit_rate(
    value: &str,
    location: &SourceLocation,
) -> ConfigResult<u64> {
    let (number, scale) = if let Some(value) = value.strip_suffix("r/s") {
        (value, 1_u64)
    } else if let Some(value) = value.strip_suffix("r/m") {
        (value, 60_u64)
    } else {
        (value, 1_u64)
    };
    let rate = number
        .parse::<u64>()
        .map_err(|_| ConfigError::at(location.clone(), format!("invalid rate \"{value}\"")))?;
    if rate == 0 {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid rate \"{value}\""),
        ));
    }
    Ok((rate.saturating_mul(1000) / scale).max(1))
}

pub(in crate::config::compile) fn parse_limit_status(
    directive: &Directive,
    name: &str,
) -> ConfigResult<u16> {
    validate_no_block(directive)?;
    let status = one_arg(directive)?.parse::<u16>().map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid status in \"{name}\" directive"),
        )
    })?;
    if !(400..=599).contains(&status) {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid status in \"{name}\" directive"),
        ));
    }
    Ok(status)
}

pub(in crate::config::compile) fn parse_limit_log_level(
    directive: &Directive,
    name: &str,
) -> ConfigResult<LogLevel> {
    validate_no_block(directive)?;
    let level = one_arg(directive)?;
    match level {
        "info" => Ok(LogLevel::Info),
        "notice" => Ok(LogLevel::Notice),
        "warn" => Ok(LogLevel::Warn),
        "error" => Ok(LogLevel::Error),
        _ => Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{level}\" in \"{name}\" directive"),
        )),
    }
}
