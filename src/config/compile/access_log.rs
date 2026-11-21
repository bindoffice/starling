use super::*;

enum ParsedAccessLog {
    Off,
    Target(AccessLogTarget),
}

pub(super) fn apply_access_log_directive(
    directive: &Directive,
    prefix: &Path,
    default_format: Option<&str>,
    config: &mut AccessLogConfig,
    seen: &mut bool,
) -> ConfigResult<()> {
    let parsed = parse_access_log(directive, prefix, default_format)?;
    if !*seen {
        *config = AccessLogConfig::default();
        *seen = true;
    }
    match parsed {
        ParsedAccessLog::Off => config.off = true,
        ParsedAccessLog::Target(target) => config.logs.push(target),
    }
    Ok(())
}

pub(super) fn default_http_access_log(prefix: &Path) -> AccessLogConfig {
    AccessLogConfig {
        off: false,
        logs: vec![AccessLogTarget {
            destination: AccessLogDestination::File(AccessLogPath::Fixed(
                prefix.join("logs/access.log"),
            )),
            format_name: "combined".into(),
            condition: None,
            buffer: None,
        }],
    }
}

pub(super) fn validate_access_log_formats(
    config: &AccessLogConfig,
    formats: &HashMap<String, AccessLogFormat>,
    allow_combined: bool,
    location: &SourceLocation,
) -> ConfigResult<()> {
    for log in &config.logs {
        if allow_combined && log.format_name.eq_ignore_ascii_case("combined") {
            continue;
        }
        if !formats
            .keys()
            .any(|name| name.eq_ignore_ascii_case(&log.format_name))
        {
            return Err(ConfigError::at(
                location.clone(),
                format!("unknown log format \"{}\"", log.format_name),
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_access_log_buffers(
    http: Option<&HttpConfig>,
    stream: Option<&StreamConfig>,
) -> ConfigResult<()> {
    let mut buffered_paths = HashMap::new();
    if let Some(http) = http {
        for server in &http.servers {
            collect_access_log_buffers(&server.context, &mut buffered_paths)?;
            for location in &server.locations {
                collect_location_access_log_buffers(location, &mut buffered_paths)?;
            }
        }
    }
    if let Some(stream) = stream {
        for server in &stream.servers {
            collect_access_log_target_buffers(&server.access_log, &mut buffered_paths)?;
        }
    }
    Ok(())
}

fn collect_location_access_log_buffers(
    location: &LocationConfig,
    buffered_paths: &mut HashMap<PathBuf, AccessLogBufferConfig>,
) -> ConfigResult<()> {
    collect_access_log_buffers(&location.context, buffered_paths)?;
    for nested in &location.nested {
        collect_location_access_log_buffers(nested, buffered_paths)?;
    }
    Ok(())
}

fn collect_access_log_buffers(
    context: &EffectiveConfig,
    buffered_paths: &mut HashMap<PathBuf, AccessLogBufferConfig>,
) -> ConfigResult<()> {
    collect_access_log_target_buffers(&context.access_log, buffered_paths)?;
    if let Some(limit_except) = &context.limit_except {
        collect_access_log_target_buffers(&limit_except.access_log, buffered_paths)?;
    }
    for conditional in &context.conditionals {
        collect_access_log_target_buffers(&conditional.access_log, buffered_paths)?;
    }
    Ok(())
}

fn collect_access_log_target_buffers(
    config: &AccessLogConfig,
    buffered_paths: &mut HashMap<PathBuf, AccessLogBufferConfig>,
) -> ConfigResult<()> {
    for log in &config.logs {
        let (Some(path), Some(buffer)) = (log.destination.as_fixed_path(), &log.buffer) else {
            continue;
        };
        if let Some(existing) = buffered_paths.get(path) {
            if existing != buffer {
                return Err(ConfigError::new(format!(
                    "access_log \"{}\" already defined with conflicting parameters",
                    path.display()
                )));
            }
        } else {
            buffered_paths.insert(path.to_path_buf(), buffer.clone());
        }
    }
    Ok(())
}

fn parse_access_log(
    directive: &Directive,
    prefix: &Path,
    default_format: Option<&str>,
) -> ConfigResult<ParsedAccessLog> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"access_log\" directive",
        ));
    }
    let path = &directive.args[0];
    if path == "off" {
        if let Some(parameter) = directive.args.get(1) {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("invalid parameter \"{parameter}\" in \"access_log\" directive"),
            ));
        }
        return Ok(ParsedAccessLog::Off);
    }
    let destination = if path.starts_with("syslog:") {
        AccessLogDestination::Syslog(parse_syslog_config(path, &directive.location)?)
    } else {
        AccessLogDestination::File(resolve_access_log_path(prefix, path))
    };
    let format_name = match directive.args.get(1) {
        Some(name) => name.clone(),
        None => match default_format {
            Some(name) => name.to_string(),
            None => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "log format is not specified",
                ));
            }
        },
    };
    let mut condition = None;
    let mut buffer_size = None;
    let mut flush = None;
    let mut gzip_level = None;
    for parameter in directive.args.iter().skip(2) {
        if let Some(value) = parameter.strip_prefix("if=") {
            condition = Some(value.to_string());
            continue;
        }
        if let Some(value) = parameter.strip_prefix("buffer=") {
            let size = parse_size(value, &directive.location).map_err(|_| {
                ConfigError::at(
                    directive.location.clone(),
                    format!("invalid buffer size \"{value}\""),
                )
            })?;
            if size == 0 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid buffer size \"{value}\""),
                ));
            }
            buffer_size = Some(usize::try_from(size).map_err(|_| {
                ConfigError::at(
                    directive.location.clone(),
                    format!("invalid buffer size \"{value}\""),
                )
            })?);
            continue;
        }
        if let Some(value) = parameter.strip_prefix("flush=") {
            if value.is_empty() {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid flush time \"{value}\""),
                ));
            }
            let duration = parse_duration(value, &directive.location).map_err(|_| {
                ConfigError::at(
                    directive.location.clone(),
                    format!("invalid flush time \"{value}\""),
                )
            })?;
            if duration.is_zero() {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid flush time \"{value}\""),
                ));
            }
            flush = Some(duration);
            continue;
        }
        if parameter == "gzip" || parameter.starts_with("gzip=") {
            buffer_size.get_or_insert(64 * 1024);
            if parameter == "gzip" {
                gzip_level = Some(1);
            } else {
                let value = parameter.strip_prefix("gzip=").unwrap_or_default();
                let level = value
                    .parse::<u32>()
                    .ok()
                    .filter(|level| (1..=9).contains(level));
                let Some(level) = level else {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!("invalid compression level \"{value}\""),
                    ));
                };
                gzip_level = Some(level);
            }
            continue;
        }
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid parameter \"{parameter}\" in \"access_log\" directive"),
        ));
    }
    if flush.is_some() && buffer_size.is_none() {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("no buffer is defined for access_log \"{path}\""),
        ));
    }
    if buffer_size.is_some() && destination.as_template().is_some() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "buffered logs cannot have variables in name",
        ));
    }
    if buffer_size.is_some() && destination.as_syslog().is_some() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "logs to syslog cannot be buffered",
        ));
    }
    let buffer = buffer_size.map(|size| AccessLogBufferConfig {
        size,
        flush,
        gzip_level,
    });
    Ok(ParsedAccessLog::Target(AccessLogTarget {
        destination,
        format_name,
        condition,
        buffer,
    }))
}

fn resolve_access_log_path(prefix: &Path, value: &str) -> AccessLogPath {
    let resolved = resolve_path(prefix, value);
    if value.contains('$') {
        AccessLogPath::Dynamic(resolved.to_string_lossy().into_owned())
    } else {
        AccessLogPath::Fixed(resolved)
    }
}
