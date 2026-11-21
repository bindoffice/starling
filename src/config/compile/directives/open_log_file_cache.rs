use super::*;

pub(in crate::config::compile) fn try_apply_open_log_file_cache_directive(
    directive: &Directive,
    target: &mut Option<Arc<OpenLogFileCacheConfig>>,
    seen: &mut bool,
) -> ConfigResult<bool> {
    if directive.name != "open_log_file_cache" {
        return Ok(false);
    }
    validate_no_block(directive)?;
    if directive.args.is_empty() || directive.args.len() > 4 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"open_log_file_cache\" directive",
        ));
    }
    if *seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"open_log_file_cache\" directive is duplicate",
        ));
    }
    *seen = true;

    let mut max = 0;
    let mut inactive = Duration::from_secs(10);
    let mut min_uses = 1;
    let mut valid = Duration::from_secs(60);
    let mut off = false;

    for parameter in &directive.args {
        if parameter == "off" {
            off = true;
            continue;
        }
        if let Some(value) = parameter.strip_prefix("max=") {
            max = value
                .parse::<usize>()
                .map_err(|_| invalid_parameter(directive, parameter))?;
            continue;
        }
        if let Some(value) = parameter.strip_prefix("inactive=") {
            inactive = parse_duration(value, &directive.location)
                .map_err(|_| invalid_parameter(directive, parameter))?;
            continue;
        }
        if let Some(value) = parameter.strip_prefix("min_uses=") {
            min_uses = value
                .parse::<usize>()
                .map_err(|_| invalid_parameter(directive, parameter))?;
            continue;
        }
        if let Some(value) = parameter.strip_prefix("valid=") {
            valid = parse_duration(value, &directive.location)
                .map_err(|_| invalid_parameter(directive, parameter))?;
            continue;
        }
        return Err(invalid_parameter(directive, parameter));
    }

    if off {
        *target = None;
        return Ok(true);
    }
    if max == 0 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"open_log_file_cache\" must have \"max\" parameter",
        ));
    }
    *target = Some(Arc::new(OpenLogFileCacheConfig {
        max,
        inactive,
        min_uses,
        valid,
    }));
    Ok(true)
}

fn invalid_parameter(directive: &Directive, parameter: &str) -> ConfigError {
    ConfigError::at(
        directive.location.clone(),
        format!("invalid \"open_log_file_cache\" parameter \"{parameter}\""),
    )
}
