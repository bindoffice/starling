use super::super::*;

pub(in crate::config::compile) fn try_apply_compat_directive(
    directive: &Directive,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "sendfile_max_chunk"
        | "subrequest_output_buffer_size"
        | "tcp_nopush"
        | "gzip_buffers"
        | "gunzip_buffers"
        | "open_file_cache"
        | "open_file_cache_valid"
        | "open_file_cache_min_uses"
        | "open_file_cache_errors"
        | "open_file_cache_events"
        | "read_ahead"
        | "aio"
        | "aio_write"
        | "directio"
        | "directio_alignment"
        | "output_buffers"
        | "postpone_output"
        | "ssl_session_cache"
        | "ssl_session_timeout"
        | "ssl_session_tickets"
        | "ssl_dhparam"
        | "ssl_ecdh_curve"
        | "ssl_early_data"
        | "ssl_buffer_size"
        | "proxy_connection_drop"
        | "client_body_early_read"
        | "connection_pool_size"
        | "request_pool_size"
        | "types_hash_max_size"
        | "types_hash_bucket_size"
        | "variables_hash_max_size"
        | "variables_hash_bucket_size"
        | "server_names_hash_max_size"
        | "server_names_hash_bucket_size"
        | "referer_hash_max_size"
        | "referer_hash_bucket_size"
        | "map_hash_max_size"
        | "map_hash_bucket_size" => {
            validate_no_block(directive)?;
            Ok(true)
        }
        "send_lowat" | "proxy_send_lowat" | "fastcgi_send_lowat" | "postpone_gzipping" => {
            validate_size_argument(directive)?;
            Ok(true)
        }
        "gzip_window" => {
            validate_size_choice(
                directive,
                &[512, 1 << 10, 1 << 11, 1 << 12, 1 << 13, 1 << 14, 1 << 15],
            )?;
            Ok(true)
        }
        "gzip_hash" => {
            validate_size_choice(
                directive,
                &[
                    512,
                    1 << 10,
                    1 << 11,
                    1 << 12,
                    1 << 13,
                    1 << 14,
                    1 << 15,
                    1 << 16,
                    1 << 17,
                ],
            )?;
            Ok(true)
        }
        "gzip_no_buffer" => {
            parse_flag(directive, "gzip_no_buffer")?;
            Ok(true)
        }
        "ssl_certificate_cache" | "proxy_ssl_certificate_cache" | "uwsgi_ssl_certificate_cache" => {
            validate_certificate_cache(directive)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn validate_size_argument(directive: &Directive) -> ConfigResult<()> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    parse_size(value, &directive.location)
        .map(|_| ())
        .map_err(|_| {
            ConfigError::at(
                directive.location.clone(),
                format!(
                    "invalid value \"{value}\" in \"{}\" directive",
                    directive.name
                ),
            )
        })
}

fn validate_size_choice(directive: &Directive, choices: &[u64]) -> ConfigResult<()> {
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
    if choices.contains(&size) {
        return Ok(());
    }
    Err(ConfigError::at(
        directive.location.clone(),
        format!(
            "invalid value \"{value}\" in \"{}\" directive",
            directive.name
        ),
    ))
}

pub(in crate::config::compile) fn validate_certificate_cache(
    directive: &Directive,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid number of arguments in \"{}\" directive",
                directive.name
            ),
        ));
    }

    let mut has_max = false;
    let mut disabled = false;
    for value in &directive.args {
        if value == "off" {
            disabled = true;
            continue;
        }
        if let Some(value) = value.strip_prefix("max=") {
            let max = value.parse::<usize>().map_err(|_| {
                ConfigError::at(
                    directive.location.clone(),
                    format!(
                        "invalid parameter \"{value}\" in \"{}\" directive",
                        directive.name
                    ),
                )
            })?;
            if max == 0 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!(
                        "invalid parameter \"max={value}\" in \"{}\" directive",
                        directive.name
                    ),
                ));
            }
            has_max = true;
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
            format!(
                "invalid parameter \"{value}\" in \"{}\" directive",
                directive.name
            ),
        ));
    }

    if !has_max && !disabled {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" must have the \"max\" parameter", directive.name),
        ));
    }
    Ok(())
}
