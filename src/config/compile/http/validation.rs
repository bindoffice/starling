use super::*;

pub(super) fn validate_http_servers(
    servers: &[ServerConfig],
    cache_paths: &HashMap<String, Arc<ProxyCachePathConfig>>,
    log_formats: &HashMap<String, AccessLogFormat>,
    location: &SourceLocation,
) -> ConfigResult<()> {
    for server in servers {
        validate_cache_zone_tree(&server.context, cache_paths, location)?;
        for location_config in &server.locations {
            validate_proxy_cache_location_tree(location_config, cache_paths, location)?;
        }
    }
    for server in servers {
        validate_http_access_log_formats(server, log_formats, location)?;
    }
    Ok(())
}

fn validate_cache_zone_tree(
    context: &EffectiveConfig,
    zones: &HashMap<String, Arc<ProxyCachePathConfig>>,
    location: &SourceLocation,
) -> ConfigResult<()> {
    validate_cache_zones(context, zones, location)
}

fn validate_proxy_cache_location_tree(
    location_config: &LocationConfig,
    zones: &HashMap<String, Arc<ProxyCachePathConfig>>,
    source: &SourceLocation,
) -> ConfigResult<()> {
    validate_cache_zone_tree(&location_config.context, zones, source)?;
    for nested in &location_config.nested {
        validate_proxy_cache_location_tree(nested, zones, source)?;
    }
    Ok(())
}

fn validate_http_access_log_formats(
    server: &ServerConfig,
    formats: &HashMap<String, AccessLogFormat>,
    location: &SourceLocation,
) -> ConfigResult<()> {
    validate_context_access_log_formats(&server.context, formats, location)?;
    for location_config in &server.locations {
        validate_location_access_log_formats(location_config, formats, location)?;
    }
    Ok(())
}

fn validate_location_access_log_formats(
    location_config: &LocationConfig,
    formats: &HashMap<String, AccessLogFormat>,
    source: &SourceLocation,
) -> ConfigResult<()> {
    validate_context_access_log_formats(&location_config.context, formats, source)?;
    for nested in &location_config.nested {
        validate_location_access_log_formats(nested, formats, source)?;
    }
    Ok(())
}

fn validate_context_access_log_formats(
    context: &EffectiveConfig,
    formats: &HashMap<String, AccessLogFormat>,
    location: &SourceLocation,
) -> ConfigResult<()> {
    validate_access_log_formats(&context.access_log, formats, true, location)?;
    if let Some(limit_except) = &context.limit_except {
        validate_access_log_formats(&limit_except.access_log, formats, true, location)?;
    }
    for conditional in &context.conditionals {
        validate_access_log_formats(&conditional.access_log, formats, true, location)?;
    }
    Ok(())
}
