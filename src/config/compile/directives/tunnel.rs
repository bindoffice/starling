use super::*;
use std::collections::HashSet;

#[derive(Default)]
pub(super) struct TunnelSeen {
    directives: HashSet<&'static str>,
}

impl TunnelSeen {
    fn mark(&mut self, directive: &Directive) -> ConfigResult<()> {
        let name = match directive.name.as_str() {
            "tunnel_bind" => "tunnel_bind",
            "tunnel_socket_keepalive" => "tunnel_socket_keepalive",
            "tunnel_socket_rcvbuf" => "tunnel_socket_rcvbuf",
            "tunnel_socket_sndbuf" => "tunnel_socket_sndbuf",
            "tunnel_connect_timeout" => "tunnel_connect_timeout",
            "tunnel_send_timeout" => "tunnel_send_timeout",
            "tunnel_read_timeout" => "tunnel_read_timeout",
            "tunnel_send_lowat" => "tunnel_send_lowat",
            "tunnel_buffer_size" => "tunnel_buffer_size",
            "tunnel_next_upstream" => "tunnel_next_upstream",
            "tunnel_next_upstream_tries" => "tunnel_next_upstream_tries",
            "tunnel_next_upstream_timeout" => "tunnel_next_upstream_timeout",
            _ => return Ok(()),
        };
        if !self.directives.insert(name) {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("\"{name}\" directive is duplicate"),
            ));
        }
        Ok(())
    }
}

pub(in crate::config::compile) fn parse_tunnel_pass(
    directive: &Directive,
) -> ConfigResult<TunnelPassConfig> {
    validate_no_block(directive)?;
    if directive.args.len() > 1 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"tunnel_pass\" directive",
        ));
    }
    Ok(TunnelPassConfig {
        raw: directive
            .args
            .first()
            .cloned()
            .unwrap_or_else(|| "$host:$request_port".into()),
    })
}

pub(super) fn try_apply_tunnel_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut TunnelSeen,
) -> ConfigResult<bool> {
    if !matches!(
        directive.name.as_str(),
        "tunnel_bind"
            | "tunnel_socket_keepalive"
            | "tunnel_socket_rcvbuf"
            | "tunnel_socket_sndbuf"
            | "tunnel_connect_timeout"
            | "tunnel_send_timeout"
            | "tunnel_read_timeout"
            | "tunnel_send_lowat"
            | "tunnel_buffer_size"
            | "tunnel_next_upstream"
            | "tunnel_next_upstream_tries"
            | "tunnel_next_upstream_timeout"
    ) {
        return Ok(false);
    }
    seen.mark(directive)?;
    match directive.name.as_str() {
        "tunnel_bind" => {
            context.tunnel.bind = parse_upstream_bind(directive, "tunnel_bind")?;
        }
        "tunnel_socket_keepalive" => {
            context.tunnel.socket_keepalive = parse_flag(directive, "tunnel_socket_keepalive")?;
        }
        "tunnel_socket_rcvbuf" => {
            context.tunnel.socket_rcvbuf =
                parse_tunnel_size(directive, "tunnel_socket_rcvbuf", true)?;
        }
        "tunnel_socket_sndbuf" => {
            context.tunnel.socket_sndbuf =
                parse_tunnel_size(directive, "tunnel_socket_sndbuf", true)?;
        }
        "tunnel_connect_timeout" => {
            context.tunnel.connect_timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "tunnel_send_timeout" => {
            context.tunnel.send_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "tunnel_read_timeout" => {
            context.tunnel.read_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "tunnel_send_lowat" => {
            context.tunnel.send_lowat = parse_tunnel_size(directive, "tunnel_send_lowat", false)?;
        }
        "tunnel_buffer_size" => {
            context.tunnel.buffer_size = parse_tunnel_size(directive, "tunnel_buffer_size", true)?;
        }
        "tunnel_next_upstream" => {
            apply_tunnel_next_upstream(directive, &mut context.tunnel.next_upstream)?;
        }
        "tunnel_next_upstream_tries" => {
            context.tunnel.next_upstream.tries =
                parse_next_upstream_tries(directive, "tunnel_next_upstream_tries")?;
        }
        "tunnel_next_upstream_timeout" => {
            context.tunnel.next_upstream.timeout =
                parse_next_upstream_timeout(directive, "tunnel_next_upstream_timeout")?;
        }
        _ => unreachable!("tunnel directive was classified above"),
    }
    Ok(true)
}

fn parse_tunnel_size(directive: &Directive, name: &str, nonzero: bool) -> ConfigResult<usize> {
    let value = one_arg(directive)?;
    let size = parse_size(value, &directive.location).map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"{name}\" directive"),
        )
    })?;
    let size = usize::try_from(size).map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"{name}\" directive"),
        )
    })?;
    if nonzero && size == 0 {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"{name}\" directive"),
        ));
    }
    Ok(size)
}

fn apply_tunnel_next_upstream(
    directive: &Directive,
    config: &mut NextUpstreamConfig,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"tunnel_next_upstream\" directive",
        ));
    }
    let mut flags = 0;
    let mut off = false;
    for parameter in &directive.args {
        match parameter.as_str() {
            "error" => flags |= NEXT_UPSTREAM_ERROR,
            "timeout" => flags |= NEXT_UPSTREAM_TIMEOUT,
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
