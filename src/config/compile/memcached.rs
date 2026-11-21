use super::*;

pub(super) fn try_apply_memcached_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    location_scope: bool,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "memcached_pass" => {
            validate_no_block(directive)?;
            if !location_scope {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"memcached_pass\" directive is not allowed here",
                ));
            }
            if context.memcached_pass.is_some() {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"memcached_pass\" directive is duplicate",
                ));
            }
            context.memcached_pass = Some(MemcachedPassConfig {
                raw: parse_gateway_pass(directive, "memcached")?,
            });
            context.tunnel.pass = None;
        }
        "memcached_bind" => {
            context.memcached_bind = common::parse_upstream_bind(directive, "memcached_bind")?;
        }
        "memcached_socket_keepalive" => {
            context.memcached_socket_keepalive =
                parse_flag(directive, "memcached_socket_keepalive")?;
        }
        "memcached_connect_timeout" => {
            validate_no_block(directive)?;
            context.memcached_connect_timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "memcached_send_timeout" => {
            validate_no_block(directive)?;
            context.memcached_send_timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "memcached_read_timeout" => {
            validate_no_block(directive)?;
            context.memcached_read_timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "memcached_buffer_size" => {
            context.memcached_buffer_size = parse_memcached_buffer_size(directive)?;
        }
        "memcached_next_upstream" => {
            apply_memcached_next_upstream(directive, &mut context.memcached_next_upstream)?;
        }
        "memcached_next_upstream_tries" => {
            context.memcached_next_upstream.tries =
                parse_next_upstream_tries(directive, "memcached_next_upstream_tries")?;
        }
        "memcached_next_upstream_timeout" => {
            context.memcached_next_upstream.timeout =
                parse_next_upstream_timeout(directive, "memcached_next_upstream_timeout")?;
        }
        "memcached_gzip_flag" => {
            context.memcached_gzip_flag = parse_memcached_gzip_flag(directive)?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn parse_memcached_buffer_size(directive: &Directive) -> ConfigResult<usize> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    let size = parse_size(value, &directive.location).map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"memcached_buffer_size\" directive"),
        )
    })?;
    if size == 0 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid value \"0\" in \"memcached_buffer_size\" directive",
        ));
    }
    usize::try_from(size).map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"memcached_buffer_size\" directive"),
        )
    })
}

fn parse_memcached_gzip_flag(directive: &Directive) -> ConfigResult<u32> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    value.parse::<u32>().map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"memcached_gzip_flag\" directive"),
        )
    })
}

fn apply_memcached_next_upstream(
    directive: &Directive,
    config: &mut NextUpstreamConfig,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"memcached_next_upstream\" directive",
        ));
    }
    let mut flags = 0;
    let mut off = false;
    for parameter in &directive.args {
        match parameter.as_str() {
            "error" => flags |= NEXT_UPSTREAM_ERROR,
            "timeout" => flags |= NEXT_UPSTREAM_TIMEOUT,
            "invalid_response" => flags |= NEXT_UPSTREAM_INVALID_HEADER,
            "not_found" => flags |= NEXT_UPSTREAM_HTTP_404,
            "off" => off = true,
            _ => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid parameter \"{parameter}\""),
                ));
            }
        }
    }
    config.flags = if off { NEXT_UPSTREAM_OFF } else { flags };
    Ok(())
}
