use super::*;

use super::super::compat::reject_ssl_conf_command;

pub(super) fn try_apply_grpc_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    prefix: &Path,
    allow_pass: bool,
    seen: &mut GrpcSeen,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "grpc_pass" => {
            if !allow_pass {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"grpc_pass\" directive is not allowed here",
                ));
            }
            validate_no_block(directive)?;
            if context.grpc.pass.is_some() {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"grpc_pass\" directive is duplicate",
                ));
            }
            context.grpc.pass = Some(parse_grpc_pass(directive)?);
            context.tunnel.pass = None;
        }
        "grpc_set_header" => {
            validate_no_block(directive)?;
            if !seen.header {
                context.grpc.headers.clear();
                seen.header = true;
            }
            if directive.args.len() != 2 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid number of arguments in \"grpc_set_header\" directive",
                ));
            }
            context.grpc.headers.push(HeaderValue {
                name: directive.args[0].clone(),
                value: directive.args[1].clone(),
            });
        }
        "grpc_hide_header" => apply_header_list(
            directive,
            &mut context.grpc.hide_headers,
            &mut seen.hide_header,
        )?,
        "grpc_pass_header" => apply_header_list(
            directive,
            &mut context.grpc.pass_headers,
            &mut seen.pass_header,
        )?,
        "grpc_ignore_headers" => apply_ignore_headers(
            directive,
            &mut context.grpc.ignore_headers,
            &mut seen.ignore_headers,
        )?,
        "grpc_connect_timeout" => {
            validate_no_block(directive)?;
            context.grpc.connect_timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "grpc_send_timeout" => {
            validate_no_block(directive)?;
            context.grpc.send_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "grpc_read_timeout" => {
            validate_no_block(directive)?;
            context.grpc.read_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "grpc_buffer_size" => {
            validate_no_block(directive)?;
            let value = one_arg(directive)?;
            let size = parse_size(value, &directive.location).map_err(|_| {
                ConfigError::at(
                    directive.location.clone(),
                    format!("invalid value \"{value}\" in \"grpc_buffer_size\" directive"),
                )
            })?;
            if size == 0 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"grpc_buffer_size\" must be greater than zero",
                ));
            }
            context.grpc.buffer_size = usize::try_from(size).map_err(|_| {
                ConfigError::at(
                    directive.location.clone(),
                    format!("invalid value \"{value}\" in \"grpc_buffer_size\" directive"),
                )
            })?;
        }
        "grpc_request_buffering" => {
            if seen.request_buffering {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"grpc_request_buffering\" directive is duplicate",
                ));
            }
            context.grpc.request_buffering = parse_flag(directive, "grpc_request_buffering")?;
            seen.request_buffering = true;
        }
        "grpc_ignore_client_abort" => {
            if seen.ignore_client_abort {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"grpc_ignore_client_abort\" directive is duplicate",
                ));
            }
            context.grpc.ignore_client_abort = parse_flag(directive, "grpc_ignore_client_abort")?;
            seen.ignore_client_abort = true;
        }
        "grpc_intercept_errors" => {
            context.grpc.intercept_errors = parse_flag(directive, "grpc_intercept_errors")?;
        }
        "grpc_next_upstream" => apply_next_upstream(directive, &mut context.grpc.next_upstream)?,
        "grpc_next_upstream_tries" => {
            context.grpc.next_upstream.tries =
                parse_next_upstream_tries(directive, "grpc_next_upstream_tries")?;
        }
        "grpc_next_upstream_timeout" => {
            context.grpc.next_upstream.timeout =
                parse_next_upstream_timeout(directive, "grpc_next_upstream_timeout")?;
        }
        "grpc_bind" => {
            context.grpc.bind = parse_upstream_bind(directive, "grpc_bind")?;
        }
        "grpc_socket_keepalive" => {
            context.grpc.socket_keepalive = parse_flag(directive, "grpc_socket_keepalive")?;
        }
        "grpc_socket_rcvbuf" => {
            context.grpc.socket_rcvbuf = parse_grpc_socket_buffer(directive)?;
        }
        "grpc_socket_sndbuf" => {
            context.grpc.socket_sndbuf = parse_grpc_socket_buffer(directive)?;
        }
        "grpc_ssl_server_name" => {
            context.grpc.ssl_server_name = parse_flag(directive, "grpc_ssl_server_name")?;
        }
        "grpc_ssl_name" => {
            validate_no_block(directive)?;
            context.grpc.ssl_name = Some(one_arg(directive)?.to_string());
        }
        "grpc_ssl_session_reuse" => {
            context.grpc.ssl_session_reuse = parse_flag(directive, "grpc_ssl_session_reuse")?;
        }
        "grpc_ssl_verify" => {
            context.grpc.ssl_verify = parse_flag(directive, "grpc_ssl_verify")?;
        }
        "grpc_ssl_trusted_certificate" => {
            validate_no_block(directive)?;
            context.grpc.ssl_trusted_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "grpc_ssl_verify_depth" => {
            validate_no_block(directive)?;
            context.grpc.ssl_verify_depth = parse_usize(
                one_arg(directive)?,
                &directive.location,
                "grpc_ssl_verify_depth",
            )? as u32;
        }
        "grpc_ssl_crl" => {
            validate_no_block(directive)?;
            context.grpc.ssl_crl = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "grpc_ssl_certificate" => {
            validate_no_block(directive)?;
            context.grpc.ssl_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "grpc_ssl_certificate_key" => {
            validate_no_block(directive)?;
            context.grpc.ssl_certificate_key = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "grpc_ssl_password_file" => {
            context.grpc.ssl_passwords = parse_password_file(prefix, directive)?;
        }
        "grpc_ssl_protocols" => {
            apply_tls_protocols(
                directive,
                &mut context.grpc.ssl_protocols,
                &mut seen.ssl_protocols,
            )?;
        }
        "grpc_ssl_ciphers" => {
            context.grpc.ssl_ciphers = parse_tls_cipher_config(directive)?;
        }
        "grpc_ssl_conf_command" => {
            reject_ssl_conf_command(directive, "grpc_ssl_conf_command")?;
        }
        "grpc_ssl_certificate_cache" => {
            validate_grpc_certificate_cache(directive)?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn parse_grpc_socket_buffer(directive: &Directive) -> ConfigResult<usize> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    let size = parse_size(value, &directive.location).map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid value \"{value}\" in \"{}\" directive",
                directive.name
            ),
        )
    })?;
    usize::try_from(size).map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid value \"{value}\" in \"{}\" directive",
                directive.name
            ),
        )
    })
}

fn validate_grpc_certificate_cache(directive: &Directive) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if !(1..=3).contains(&directive.args.len()) {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"grpc_ssl_certificate_cache\" directive",
        ));
    }
    let mut max_seen = false;
    for value in &directive.args {
        if value == "off" {
            continue;
        }
        if let Some(value) = value.strip_prefix("max=") {
            let max = value.parse::<usize>().map_err(|_| {
                ConfigError::at(
                    directive.location.clone(),
                    format!("invalid parameter \"{value}\""),
                )
            })?;
            if max == 0 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid parameter \"max={value}\""),
                ));
            }
            max_seen = true;
            continue;
        }
        if let Some(value) = value
            .strip_prefix("inactive=")
            .or_else(|| value.strip_prefix("valid="))
        {
            parse_time_seconds(value, &directive.location)?;
            continue;
        }
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid parameter \"{value}\""),
        ));
    }
    if !max_seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"grpc_ssl_certificate_cache\" must have the \"max\" parameter",
        ));
    }
    Ok(())
}
