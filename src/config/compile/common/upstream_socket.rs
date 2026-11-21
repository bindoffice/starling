use super::*;

pub(super) fn try_apply_upstream_socket_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "proxy_bind" => {
            context.proxy_bind = parse_upstream_bind(directive, "proxy_bind")?;
        }
        "proxy_socket_keepalive" => {
            context.proxy_socket_keepalive = parse_flag(directive, "proxy_socket_keepalive")?;
        }
        "proxy_socket_rcvbuf" => {
            context.proxy_socket_rcvbuf =
                parse_socket_buffer_size(directive, "proxy_socket_rcvbuf")?;
        }
        "proxy_socket_sndbuf" => {
            context.proxy_socket_sndbuf =
                parse_socket_buffer_size(directive, "proxy_socket_sndbuf")?;
        }
        "fastcgi_bind" => {
            context.fastcgi_bind = parse_upstream_bind(directive, "fastcgi_bind")?;
        }
        "fastcgi_socket_keepalive" => {
            context.fastcgi_socket_keepalive = parse_flag(directive, "fastcgi_socket_keepalive")?;
        }
        "fastcgi_socket_rcvbuf" => {
            context.fastcgi_socket_rcvbuf =
                parse_socket_buffer_size(directive, "fastcgi_socket_rcvbuf")?;
        }
        "fastcgi_socket_sndbuf" => {
            context.fastcgi_socket_sndbuf =
                parse_socket_buffer_size(directive, "fastcgi_socket_sndbuf")?;
        }
        "uwsgi_bind" => {
            context.uwsgi_bind = parse_upstream_bind(directive, "uwsgi_bind")?;
        }
        "uwsgi_socket_keepalive" => {
            context.uwsgi_socket_keepalive = parse_flag(directive, "uwsgi_socket_keepalive")?;
        }
        "uwsgi_socket_rcvbuf" => {
            context.uwsgi_socket_rcvbuf =
                parse_socket_buffer_size(directive, "uwsgi_socket_rcvbuf")?;
        }
        "uwsgi_socket_sndbuf" => {
            context.uwsgi_socket_sndbuf =
                parse_socket_buffer_size(directive, "uwsgi_socket_sndbuf")?;
        }
        "scgi_bind" => {
            context.scgi_bind = parse_upstream_bind(directive, "scgi_bind")?;
        }
        "scgi_socket_keepalive" => {
            context.scgi_socket_keepalive = parse_flag(directive, "scgi_socket_keepalive")?;
        }
        "scgi_socket_rcvbuf" => {
            context.scgi_socket_rcvbuf = parse_socket_buffer_size(directive, "scgi_socket_rcvbuf")?;
        }
        "scgi_socket_sndbuf" => {
            context.scgi_socket_sndbuf = parse_socket_buffer_size(directive, "scgi_socket_sndbuf")?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

pub(in crate::config::compile) fn parse_upstream_bind(
    directive: &Directive,
    name: &str,
) -> ConfigResult<Option<String>> {
    validate_no_block(directive)?;
    if !matches!(directive.args.len(), 1 | 2) {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid number of arguments in \"{name}\" directive"),
        ));
    }
    let address = &directive.args[0];
    if directive.args.len() == 2 {
        if directive.args[1] != "transparent" {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!(
                    "invalid parameter \"{}\" in \"{name}\" directive",
                    directive.args[1]
                ),
            ));
        }
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("transparent proxying is not supported in \"{name}\" directive"),
        ));
    }
    if address == "off" {
        return Ok(None);
    }
    if !address.contains('$') && !is_valid_upstream_bind_literal(address) {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid address \"{address}\" in \"{name}\" directive"),
        ));
    }
    Ok(Some(address.clone()))
}

fn is_valid_upstream_bind_literal(value: &str) -> bool {
    if value.parse::<IpAddr>().is_ok() {
        return true;
    }
    if value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .is_some_and(|value| value.parse::<IpAddr>().is_ok())
    {
        return true;
    }
    value
        .parse::<SocketAddr>()
        .is_ok_and(|address| address.port() != 0)
}

fn parse_socket_buffer_size(directive: &Directive, name: &str) -> ConfigResult<usize> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    let size = parse_size(value, &directive.location).map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"{name}\" directive"),
        )
    })?;
    usize::try_from(size).map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"{name}\" directive"),
        )
    })
}
